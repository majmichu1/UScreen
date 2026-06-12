package com.uscreen

import android.media.MediaCodec
import android.media.MediaFormat
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
    }

    private var socket: Socket? = null
    private var inputStream: InputStream? = null
    private var mediaCodec: MediaCodec? = null
    private var outputThread: Thread? = null
    @Volatile private var isRunning = false
    @Volatile private var codecAlive = false

    var onConnected: (() -> Unit)? = null
    var onDisconnected: (() -> Unit)? = null

    /** Initial decoder format hint; the decoder adapts to the SPS anyway. */
    @Volatile var formatWidth = 1920
    @Volatile var formatHeight = 1080

    // Stats
    private val frameCounter = AtomicInteger(0)
    private val byteCounter = AtomicLong(0)
    @Volatile var currentFps = 0f; private set
    @Volatile var currentMbps = 0f; private set

    private val surfaceReady = AtomicBoolean(false)
    private val pendingSurface = AtomicReference<Surface?>(null)

    private val job = SupervisorJob()
    private val scope = CoroutineScope(Dispatchers.IO + job)

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

    private fun setupCodec(surface: Surface): Boolean {
        try {
            val format = MediaFormat.createVideoFormat(MIME_TYPE, formatWidth, formatHeight)
            format.setInteger(MediaFormat.KEY_FRAME_RATE, 60)
            format.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)

            // Low latency flags (safe to set, ignored if unsupported)
            try {
                format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            } catch (_: Exception) {}
            try {
                format.setInteger("operating-rate", 120)
            } catch (_: Exception) {}
            try {
                format.setInteger("vendor.qti-ext-dec-low-latency.enable", 1)
            } catch (_: Exception) {}

            val codec = MediaCodec.createDecoderByType(MIME_TYPE)
            codec.configure(format, surface, null, 0)
            codec.setVideoScalingMode(MediaCodec.VIDEO_SCALING_MODE_SCALE_TO_FIT)
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
                        codec.releaseOutputBuffer(index, true)
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
        isRunning = true
        scope.launch {
            connectAndReceive()
        }

        scope.launch {
            while (isRunning) {
                delay(1000)
                currentFps = frameCounter.getAndSet(0).toFloat()
                currentMbps = byteCounter.getAndSet(0) * 8f / 1_000_000f
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
                    receiveBufferSize = 1 shl 20
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
                    val payloadSize = frameSize - 1
                    when (packetType) {
                        PACKET_TYPE_CONFIG -> {
                            Log.i(TAG, "Received codec config: ${payloadSize}B")
                            feedDecoder(codec, packetBuf, 1, payloadSize, true)
                        }
                        PACKET_TYPE_FRAME -> {
                            if (firstFrame) {
                                firstFrame = false
                                onConnected?.invoke()
                            }
                            feedDecoder(codec, packetBuf, 1, payloadSize, false)
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
    private fun feedDecoder(codec: MediaCodec, data: ByteArray, offset: Int, size: Int, isConfig: Boolean) {
        try {
            var attempts = 0
            while (true) {
                val inputIndex = codec.dequeueInputBuffer(20_000) // 20ms
                if (inputIndex >= 0) {
                    val inputBuffer = codec.getInputBuffer(inputIndex) ?: return
                    inputBuffer.clear()
                    inputBuffer.put(data, offset, size)

                    val flags = if (isConfig) MediaCodec.BUFFER_FLAG_CODEC_CONFIG else 0
                    codec.queueInputBuffer(
                        inputIndex,
                        0,
                        size,
                        System.nanoTime() / 1000,
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

    private fun resetCodec() {
        synchronized(this) {
            codecAlive = false
            outputThread?.join(500)
            outputThread = null
            mediaCodec?.let {
                try { it.stop() } catch (_: Exception) {}
                try { it.release() } catch (_: Exception) {}
            }
            mediaCodec = null
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

        // Then cancel coroutines
        job.cancel()

        outputThread?.join(500)
        outputThread = null

        // Finally release codec
        mediaCodec?.let {
            try {
                it.stop()
                it.release()
            } catch (_: Exception) {}
        }
        mediaCodec = null
        surfaceReady.set(false)
        pendingSurface.set(null)
    }
}
