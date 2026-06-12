package com.uscreen

import android.util.Log
import android.view.MotionEvent
import android.view.SurfaceView
import kotlinx.coroutines.*
import okhttp3.*
import org.json.JSONObject
import java.util.concurrent.TimeUnit

class TouchCapture {
    companion object {
        const val TAG = "UScreenTouch"
        const val WS_URL = "ws://127.0.0.1:8891"
        const val RECONNECT_DELAY_MS = 2000L
    }

    private var webSocket: WebSocket? = null
    @Volatile private var isConnected = false
    private var reconnectJob: Job? = null
    private var surfaceView: SurfaceView? = null

    /** Settings to (re)send to the host whenever the control channel connects */
    @Volatile private var pendingConfig: JSONObject? = null

    /** Tablet's native landscape resolution, reported to the host on connect
     *  so the virtual display can match it automatically. */
    @Volatile private var nativeWidth = 0
    @Volatile private var nativeHeight = 0

    fun setNativeResolution(width: Int, height: Int) {
        nativeWidth = width
        nativeHeight = height
    }

    private val client = OkHttpClient.Builder()
        .readTimeout(0, TimeUnit.SECONDS)
        .connectTimeout(5, TimeUnit.SECONDS)
        .build()

    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())

    private val wsListener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            isConnected = true
            Log.i(TAG, "Connected")
            if (nativeWidth > 0 && nativeHeight > 0) {
                val res = JSONObject().apply {
                    put("type", "resolution")
                    put("width", nativeWidth)
                    put("height", nativeHeight)
                }
                webSocket.send(res.toString())
                Log.i(TAG, "Reported native resolution: ${nativeWidth}x${nativeHeight}")
            }
            pendingConfig?.let { webSocket.send(it.toString()) }
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            isConnected = false
            scheduleReconnect()
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            isConnected = false
            Log.w(TAG, "Connection failed: ${t.message}")
            scheduleReconnect()
        }
    }

    fun setSurfaceView(sv: SurfaceView) {
        surfaceView = sv

        sv.setOnTouchListener { view, event ->
            handleMotionEvent(event, view.width, view.height)
            true
        }
    }

    fun connect() {
        connectWebSocket()
    }

    private fun connectWebSocket() {
        val request = Request.Builder()
            .url(WS_URL)
            .build()
        webSocket = client.newWebSocket(request, wsListener)
    }

    private fun scheduleReconnect() {
        reconnectJob?.cancel()
        reconnectJob = scope.launch {
            delay(RECONNECT_DELAY_MS)
            if (!isConnected) {
                connectWebSocket()
            }
        }
    }

    fun handleMotionEvent(event: MotionEvent, width: Int, height: Int): Boolean {
        if (!isConnected) return false

        val vw = width.coerceAtLeast(1).toFloat()
        val vh = height.coerceAtLeast(1).toFloat()

        val pointerCount = event.pointerCount
        val actionIndex = event.actionIndex
        val maskedAction = event.actionMasked

        when (maskedAction) {
            MotionEvent.ACTION_DOWN,
            MotionEvent.ACTION_POINTER_DOWN -> {
                val isStylus = isStylus(event, actionIndex)
                if (isStylus) {
                    sendPenEvent(event, actionIndex, 0, vw, vh)
                } else {
                    sendTouch(event.getX(actionIndex) / vw,
                        event.getY(actionIndex) / vh,
                        event.getPressure(actionIndex).toDouble(),
                        0, actionIndex)
                }
            }

            MotionEvent.ACTION_MOVE -> {
                for (i in 0 until pointerCount) {
                    val isStylus = isStylus(event, i)
                    if (isStylus) {
                        sendPenEvent(event, i, 2, vw, vh)
                    } else {
                        sendTouch(event.getX(i) / vw,
                            event.getY(i) / vh,
                            event.getPressure(i).toDouble(),
                            2, i)
                    }
                }
            }

            MotionEvent.ACTION_UP,
            MotionEvent.ACTION_POINTER_UP -> {
                val isStylus = isStylus(event, actionIndex)
                if (isStylus) {
                    sendPenEvent(event, actionIndex, 1, vw, vh)
                } else {
                    sendTouch(event.getX(actionIndex) / vw,
                        event.getY(actionIndex) / vh,
                        0.0, 1, actionIndex)
                }
            }

            MotionEvent.ACTION_CANCEL -> {
                for (i in 0 until pointerCount) {
                    sendTouch(event.getX(i) / vw,
                        event.getY(i) / vh,
                        0.0, 1, i)
                }
            }
        }
        return true
    }

    private fun isStylus(event: MotionEvent, index: Int): Boolean {
        return try {
            event.getToolType(index) == MotionEvent.TOOL_TYPE_STYLUS
        } catch (_: Exception) {
            false
        }
    }

    private fun sendPenEvent(event: MotionEvent, index: Int, action: Int,
                             vw: Float, vh: Float) {
        val x = event.getX(index) / vw
        val y = event.getY(index) / vh
        val pressure = event.getPressure(index).toDouble()

        var tiltX = 0.0
        var tiltY = 0.0
        try {
            tiltX = event.getAxisValue(MotionEvent.AXIS_TILT, index).toDouble()
        } catch (_: Exception) {}
        try {
            tiltY = event.getAxisValue(MotionEvent.AXIS_ORIENTATION, index).toDouble()
        } catch (_: Exception) {}

        val msg = JSONObject().apply {
            put("type", "pen")
            put("x", x)
            put("y", y)
            put("pressure", pressure.coerceIn(0.0, 1.0))
            put("tilt_x", tiltX)
            put("tilt_y", tiltY)
            put("action", action)
        }
        webSocket?.send(msg.toString())
    }

    private fun sendTouch(x: Float, y: Float, pressure: Double,
                          action: Int, slot: Int) {
        val msg = JSONObject().apply {
            put("type", "touch")
            put("x", x.toDouble())
            put("y", y.toDouble())
            put("pressure", pressure.coerceIn(0.0, 1.0))
            put("action", action)
            put("slot", slot)
        }
        webSocket?.send(msg.toString())
    }

    /**
     * Push encoder settings to the host. The host live-restarts ffmpeg with
     * the new parameters and persists them in its config file. Settings are
     * also remembered here and re-sent on every reconnect.
     */
    fun sendConfig(bitrateKbps: Int, fps: Int) {
        val msg = JSONObject().apply {
            put("type", "config")
            put("bitrate", bitrateKbps)
            put("fps", fps)
        }
        pendingConfig = msg
        if (isConnected) {
            webSocket?.send(msg.toString())
            Log.i(TAG, "Sent config: $msg")
        }
    }

    fun isControlConnected(): Boolean = isConnected

    fun disconnect() {
        reconnectJob?.cancel()
        webSocket?.close(1000, "Client closing")
        webSocket = null
        isConnected = false
    }
}
