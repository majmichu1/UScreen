package com.uscreen

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.view.Surface
import android.view.SurfaceView
import kotlinx.coroutines.*
import java.io.InputStream
import java.net.Socket
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference

class VideoReceiver {
    companion object {
        const val HOST = "127.0.0.1"
        const val PORT = 8890
        const val MIME_TYPE = "video/avc"
        const val TAG = "UScreenVideo"
        const val MAX_FRAME_SIZE = 8 * 1024 * 1024
        const val PACKET_TYPE_CONFIG = 0
        const val PACKET_TYPE_FRAME = 1

        /** type byte + 4-byte big-endian sequence number */
        const val FRAME_HEADER_SIZE = 5

        /**
         * Acknowledge every Nth rendered frame.
         *
         * Every frame: an idle screen only sends ~5fps, so sampling one in four
         * left 6-12 measurements per report window and percentiles that moved
         * several milliseconds run to run — enough noise to read a regression
         * into pure variance. One small websocket message per frame is
         * negligible next to the pen event rate.
         */
        const val ACK_EVERY = 1

        /** Frames of arrival history kept for the decode-time split. */
        const val ARRIVAL_RING = 64
    }

    private var socket: Socket? = null
    private var inputStream: InputStream? = null
    private var mediaCodec: MediaCodec? = null
    private var outputThread: Thread? = null
    @Volatile private var isRunning = false
    @Volatile private var codecAlive = false

    var onConnected: (() -> Unit)? = null
    var onDisconnected: (() -> Unit)? = null

    /**
     * Invoked with the host's frame sequence number once that frame is
     * actually on screen. The host times the round trip on its own clock, so
     * no clock synchronisation between the two devices is needed.
     */
    var onFrameRendered: ((seq: Int, decodeUs: Int) -> Unit)? = null

    private var frameCallbackThread: HandlerThread? = null
    private val renderedCount = AtomicLong(0)

    /**
     * seq → nanoTime the frame finished arriving, so the render callback can
     * report how much of the end-to-end latency was spent on this device
     * rather than on the wire. Bounded and cheap: a plain ring, since frames
     * are rendered in the order they arrive.
     */
    private val arrivalSeq = IntArray(ARRIVAL_RING)
    private val arrivalNanos = LongArray(ARRIVAL_RING)
    @Volatile private var arrivalWrite = 0

    /**
     * Splits the on-device time into "decoder produced the frame" and "the
     * compositor put it on screen". Without this the two are indistinguishable,
     * and they call for completely different fixes — decoder settings versus
     * refresh rate and composition path.
     */
    private val releaseNanos = LongArray(ARRIVAL_RING)
    private var decodeSumUs = 0L
    private var presentSumUs = 0L
    private var splitCount = 0
    private var lastSplitLogNanos = 0L

    private fun noteReleased(seq: Int) {
        for (n in 0 until ARRIVAL_RING) {
            val i = (arrivalWrite - 1 - n + ARRIVAL_RING * 2) % ARRIVAL_RING
            if (arrivalSeq[i] == seq) {
                releaseNanos[i] = System.nanoTime()
                return
            }
        }
    }

    private fun noteArrival(seq: Int) {
        val i = arrivalWrite % ARRIVAL_RING
        arrivalSeq[i] = seq
        arrivalNanos[i] = System.nanoTime()
        arrivalWrite = arrivalWrite + 1
    }

    /** Microseconds between the frame arriving and it being on screen, or -1. */
    private fun decodeMicrosFor(seq: Int): Int {
        for (n in 0 until ARRIVAL_RING) {
            val i = (arrivalWrite - 1 - n + ARRIVAL_RING * 2) % ARRIVAL_RING
            if (arrivalSeq[i] == seq && arrivalNanos[i] != 0L) {
                val now = System.nanoTime()
                val total = ((now - arrivalNanos[i]) / 1000L)
                    .coerceIn(0L, Int.MAX_VALUE.toLong()).toInt()

                // Attribute the time: decode = arrival → buffer released,
                // present = released → actually on screen (composition+vsync).
                val rel = releaseNanos[i]
                if (rel > arrivalNanos[i]) {
                    decodeSumUs += (rel - arrivalNanos[i]) / 1000L
                    presentSumUs += (now - rel) / 1000L
                    splitCount++
                    if (now - lastSplitLogNanos > 5_000_000_000L && splitCount > 0) {
                        Log.i(
                            TAG,
                            "on-device split: decode ${decodeSumUs / splitCount / 1000.0}ms " +
                                "present ${presentSumUs / splitCount / 1000.0}ms " +
                                "($splitCount frames)"
                        )
                        lastSplitLogNanos = now
                        decodeSumUs = 0; presentSumUs = 0; splitCount = 0
                    }
                }
                return total
            }
        }
        return -1
    }

    /** Initial decoder format hint; the decoder adapts to the SPS anyway. */
    @Volatile var formatWidth = 1920
    @Volatile var formatHeight = 1080

    /** Frame rate the host is configured to send, used to size decoder hints. */
    @Volatile var streamFps = Prefs.DEFAULT_FPS

    // Stats
    private val frameCounter = AtomicInteger(0)
    private val byteCounter = AtomicLong(0)
    @Volatile var currentFps = 0f; private set
    @Volatile var currentMbps = 0f; private set

    private val surfaceReady = AtomicBoolean(false)
    private val pendingSurface = AtomicReference<Surface?>(null)

    /**
     * Recreated on every [start].
     *
     * These must NOT be `val`s initialised once: [stop] cancels the job, and a
     * cancelled [SupervisorJob] stays cancelled forever, so every later
     * `scope.launch {}` returns an already-dead coroutine whose body never
     * runs. That is what left the tablet on a black screen after the app had
     * been backgrounded once — the only cure was force-stopping it.
     */
    private var job: Job? = null
    private var scope: CoroutineScope? = null

    fun setSurface(surfaceView: SurfaceView) {
        val surface = surfaceView.holder.surface
        if (surface == null || !surface.isValid) {
            Log.w(TAG, "Surface not ready yet")
            return
        }
        pendingSurface.set(surface)
        surfaceReady.set(true)
        Log.i(TAG, "Surface stored, ready for codec setup")

        synchronized(this) {
            if (mediaCodec == null && surfaceReady.get()) {
                setupCodec(surface)
            }
        }
    }

    /**
     * The surface backing the decoder is going away. Release the codec here
     * rather than letting it keep rendering into a destroyed surface, which
     * throws from the render thread on the way to the background.
     */
    fun onSurfaceDestroyed() {
        surfaceReady.set(false)
        pendingSurface.set(null)
        releaseCodec()
    }

    private fun setupCodec(surface: Surface): Boolean {
        try {
            val format = MediaFormat.createVideoFormat(MIME_TYPE, formatWidth, formatHeight)
            // Follow the stream's real frame rate rather than a hardcoded
            // guess: telling the decoder 90 when the host sends 60 skews its
            // internal pacing and power/clock decisions.
            format.setInteger(MediaFormat.KEY_FRAME_RATE, streamFps)
            format.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)

            // State the colour space explicitly rather than relying on the SPS
            // alone. A/B measured: no latency cost either way, and being
            // explicit means the decoder cannot guess wrong.
            try {
                format.setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_LIMITED)
                format.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709)
                format.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO)
            } catch (_: Exception) {}

            // Low latency flags (safe to set, ignored if unsupported)
            try {
                format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            } catch (_: Exception) {}
            try {
                // Ask the decoder to run flat out rather than pace to the frame
                // rate — headroom above the stream rate, so a late frame is
                // caught up on instead of waiting for the next slot.
                format.setInteger("operating-rate", streamFps * 2)
            } catch (_: Exception) {}
            try {
                format.setInteger("vendor.qti-ext-dec-low-latency.enable", 1)
            } catch (_: Exception) {}

            val codec = MediaCodec.createDecoderByType(MIME_TYPE)
            codec.configure(format, surface, null, 0)
            codec.setVideoScalingMode(MediaCodec.VIDEO_SCALING_MODE_SCALE_TO_FIT)

            // Fires when a frame has actually reached the output surface —
            // the true "it is on screen" moment, rather than the earlier
            // moment we handed the buffer back. The host's sequence number
            // rides along as the presentation timestamp.
            val cbThread = HandlerThread("uscreen-frame-cb").apply { start() }
            frameCallbackThread = cbThread
            codec.setOnFrameRenderedListener({ _, presentationTimeUs, _ ->
                if (renderedCount.incrementAndGet() % ACK_EVERY == 0L) {
                    val seq = presentationTimeUs.toInt()
                    onFrameRendered?.invoke(seq, decodeMicrosFor(seq))
                }
            }, Handler(cbThread.looper))

            codec.start()
            mediaCodec = codec
            codecAlive = true
            startOutputThread(codec)
            Log.i(TAG, "Codec configured and started with surface")
            return true
        } catch (e: Exception) {
            Log.e(TAG, "Failed to setup codec", e)
            return false
        }
    }

    /**
     * Dedicated render thread: drains decoded frames and releases them to the
     * surface as soon as they're ready, independent of network reads. This is
     * what keeps the display latency at "one frame", not "one network stall".
     */
    private fun startOutputThread(codec: MediaCodec) {
        outputThread = Thread({
            val info = MediaCodec.BufferInfo()
            var rendered = 0L
            while (codecAlive) {
                try {
                    val index = codec.dequeueOutputBuffer(info, 10_000) // 10ms
                    if (index >= 0) {
                        val seq = info.presentationTimeUs.toInt()
                        codec.releaseOutputBuffer(index, true)
                        noteReleased(seq)
                        frameCounter.incrementAndGet()
                        rendered++
                        if (rendered <= 2) Log.i(TAG, "Rendered output frame #$rendered")
                    }
                } catch (e: IllegalStateException) {
                    if (codecAlive) Log.w(TAG, "Output thread: codec gone", e)
                    break
                } catch (e: Exception) {
                    if (codecAlive) Log.w(TAG, "Output thread error", e)
                }
            }
        }, "uscreen-render").apply {
            priority = Thread.MAX_PRIORITY
            start()
        }
    }

    fun start() {
        synchronized(this) {
            if (isRunning) return
            isRunning = true
            // Fresh job/scope per start — see the field docs.
            val newJob = SupervisorJob()
            val newScope = CoroutineScope(Dispatchers.IO + newJob)
            job = newJob
            scope = newScope

            newScope.launch {
                connectAndReceive()
            }

            newScope.launch {
                while (isRunning) {
                    delay(1000)
                    currentFps = frameCounter.getAndSet(0).toFloat()
                    currentMbps = byteCounter.getAndSet(0) * 8f / 1_000_000f
                }
            }
        }
    }

    private suspend fun connectAndReceive() {
        while (isRunning) {
            try {
                // Wait for surface to be ready before connecting
                while (isRunning && !surfaceReady.get()) {
                    Log.d(TAG, "Waiting for surface...")
                    delay(200)
                }
                if (!isRunning) return

                // Ensure codec is set up
                val codecReady = synchronized(this@VideoReceiver) {
                    if (mediaCodec == null) {
                        val surface = pendingSurface.get()
                        if (surface != null && surface.isValid) {
                            setupCodec(surface)
                        } else {
                            false
                        }
                    } else {
                        true
                    }
                }
                if (!codecReady) {
                    Log.w(TAG, "Codec/surface not ready, retrying...")
                    delay(500)
                    continue
                }

                Log.i(TAG, "Connecting to $HOST:$PORT...")
                socket = Socket(HOST, PORT).apply {
                    tcpNoDelay = true
                    soTimeout = 10000 // 10s read timeout
                    // Small on purpose. A 1 MB receive buffer let the host run
                    // ahead and park whole frames here, where they are pure
                    // delay that neither side can see or skip past. Keeping it
                    // shallow pushes backpressure back to the host, which does
                    // know how to drop stale frames.
                    receiveBufferSize = 128 * 1024
                }
                inputStream = socket?.getInputStream()
                Log.i(TAG, "Connected to video stream")

                val sizeHeader = ByteArray(4)
                // Reused across frames to avoid 60 allocations/s of multi-MB arrays
                var packetBuf = ByteArray(512 * 1024)
                var firstFrame = true

                receiveLoop@ while (isRunning) {
                    val codec = mediaCodec ?: break

                    readExact(inputStream!!, sizeHeader, 4)

                    val frameSize = ((sizeHeader[0].toInt() and 0xFF) shl 24) or
                            ((sizeHeader[1].toInt() and 0xFF) shl 16) or
                            ((sizeHeader[2].toInt() and 0xFF) shl 8) or
                            (sizeHeader[3].toInt() and 0xFF)

                    if (frameSize <= 1 || frameSize > MAX_FRAME_SIZE + 1) {
                        Log.w(TAG, "Invalid packet size: $frameSize, reconnecting")
                        break // Reconnect
                    }

                    if (packetBuf.size < frameSize) {
                        packetBuf = ByteArray(frameSize + frameSize / 2)
                    }
                    readExact(inputStream!!, packetBuf, frameSize)
                    byteCounter.addAndGet(frameSize.toLong())

                    val packetType = packetBuf[0].toInt() and 0xFF
                    when (packetType) {
                        PACKET_TYPE_CONFIG -> {
                            val payloadSize = frameSize - 1
                            Log.i(TAG, "Received codec config: ${payloadSize}B")
                            feedDecoder(codec, packetBuf, 1, payloadSize, true, 0L)
                        }
                        PACKET_TYPE_FRAME -> {
                            if (frameSize <= FRAME_HEADER_SIZE) {
                                Log.w(TAG, "Truncated frame packet: $frameSize, reconnecting")
                                break@receiveLoop
                            }
                            // 4-byte big-endian sequence number after the type
                            // byte, carried through the decoder as the
                            // presentation timestamp and echoed to the host.
                            val seq = ((packetBuf[1].toInt() and 0xFF) shl 24) or
                                    ((packetBuf[2].toInt() and 0xFF) shl 16) or
                                    ((packetBuf[3].toInt() and 0xFF) shl 8) or
                                    (packetBuf[4].toInt() and 0xFF)
                            if (firstFrame) {
                                firstFrame = false
                                onConnected?.invoke()
                            }
                            noteArrival(seq)
                            feedDecoder(
                                codec, packetBuf, FRAME_HEADER_SIZE,
                                frameSize - FRAME_HEADER_SIZE, false,
                                seq.toLong() and 0xFFFFFFFFL
                            )
                        }
                        else -> {
                            Log.w(TAG, "Unknown packet type: $packetType, reconnecting")
                            break@receiveLoop
                        }
                    }
                }
            } catch (e: java.io.EOFException) {
                if (isRunning) {
                    Log.i(TAG, "Stream ended (server closed)")
                    onDisconnected?.invoke()
                    delay(1000)
                }
            } catch (e: java.net.SocketTimeoutException) {
                if (isRunning) {
                    Log.w(TAG, "Stream read timeout, reconnecting")
                    onDisconnected?.invoke()
                    delay(500)
                }
            } catch (e: Exception) {
                if (isRunning) {
                    Log.e(TAG, "Stream error: ${e.message}")
                    onDisconnected?.invoke()
                    delay(1000)
                }
            } finally {
                try {
                    socket?.close()
                } catch (_: Exception) {}
                socket = null
                inputStream = null
            }
        }
    }

    /**
     * Queue one access unit into the decoder. Never silently drops frames:
     * a dropped P-frame corrupts the picture until the next keyframe. If no
     * input buffer frees up within ~200ms the codec is genuinely stuck and we
     * reset it instead.
     */
    private fun feedDecoder(
        codec: MediaCodec, data: ByteArray, offset: Int, size: Int,
        isConfig: Boolean, presentationTimeUs: Long
    ) {
        try {
            var attempts = 0
            while (true) {
                val inputIndex = codec.dequeueInputBuffer(20_000) // 20ms
                if (inputIndex >= 0) {
                    val inputBuffer = codec.getInputBuffer(inputIndex) ?: return
                    inputBuffer.clear()
                    inputBuffer.put(data, offset, size)

                    val flags = if (isConfig) MediaCodec.BUFFER_FLAG_CODEC_CONFIG else 0
                    // The host's sequence number rides in the presentation
                    // timestamp so the render callback can identify the frame.
                    codec.queueInputBuffer(
                        inputIndex,
                        0,
                        size,
                        presentationTimeUs,
                        flags
                    )
                    return
                }
                attempts++
                if (attempts >= 10) {
                    Log.w(TAG, "Decoder stuck for 200ms — resetting codec")
                    resetCodec()
                    return
                }
            }
        } catch (e: MediaCodec.CodecException) {
            Log.e(TAG, "Decoder codec error: ${e.diagnosticInfo}", e)
            resetCodec()
        } catch (e: Exception) {
            Log.w(TAG, "Decoder feed error", e)
        }
    }

    /** Tear the decoder down without touching the surface or the socket. */
    private fun releaseCodec() {
        synchronized(this) {
            codecAlive = false
            outputThread?.join(500)
            outputThread = null
            mediaCodec?.let {
                try { it.stop() } catch (_: Exception) {}
                try { it.release() } catch (_: Exception) {}
            }
            mediaCodec = null
            frameCallbackThread?.quitSafely()
            frameCallbackThread = null
        }
    }

    private fun resetCodec() {
        synchronized(this) {
            releaseCodec()
            val surface = pendingSurface.get()
            if (surface != null && surface.isValid) {
                setupCodec(surface)
            }
        }
    }

    private fun readExact(stream: InputStream, buffer: ByteArray, length: Int) {
        var offset = 0
        while (offset < length) {
            val read = stream.read(buffer, offset, length - offset)
            if (read < 0) throw java.io.EOFException("Stream closed")
            offset += read
        }
    }

    fun getFps(): Float = currentFps
    fun getMbps(): Float = currentMbps

    fun stop() {
        isRunning = false
        codecAlive = false
        // Close socket first to unblock any pending reads
        try {
            socket?.close()
        } catch (_: Exception) {}
        socket = null
        inputStream = null

        // Then cancel coroutines. The job is dropped rather than reused: a new
        // one is created by the next start().
        job?.cancel()
        job = null
        scope = null

        releaseCodec()

        // The surface is deliberately left alone. It belongs to the
        // SurfaceView, which outlives any single streaming session — it stays
        // in the view tree while the tablet is a graphics tablet, so no
        // surfaceCreated callback ever comes to hand it back. Clearing it here
        // left the next start() waiting on a surface that would never arrive,
        // showing the last decoded frame frozen on screen.
    }
}
