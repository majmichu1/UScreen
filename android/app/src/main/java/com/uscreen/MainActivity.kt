package com.uscreen

import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.SurfaceView
import android.view.View
import android.view.WindowInsetsController
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.animation.*
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.delay
import kotlin.math.roundToInt

class MainActivity : ComponentActivity() {
    /// Mirrors the host's mode so the UI can say what is going on.
    private var penOnlyMode by mutableStateOf(false)
    private var updateAvailable by mutableStateOf<String?>(null)
    private var updateChecked = false
    private var videoReceiver: VideoReceiver? = null
    private var touchCapture: TouchCapture? = null
    private lateinit var prefs: Prefs

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Keep screen on while streaming
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        requestHighestRefreshRate()

        prefs = Prefs(this)
        videoReceiver = VideoReceiver()
        touchCapture = TouchCapture()
        applyToken(intent, restart = false)

        // Close the host's latency measurement loop: every acknowledged frame
        // lets the host time capture→display on its own clock.
        videoReceiver?.onFrameRendered = { seq, decodeUs ->
            touchCapture?.sendRendered(seq, decodeUs)
        }
        videoReceiver?.streamFps = prefs.fps
        touchCapture?.onCodecKnown = { codec ->
            val mime = if (codec == "hevc") VideoReceiver.MIME_TYPE_HEVC
                       else VideoReceiver.MIME_TYPE
            val vr = videoReceiver
            if (vr != null && vr.mimeType != mime) {
                runOnUiThread {
                    // The decoder is built once per streaming session, so a
                    // codec change has to restart it. This normally fires
                    // before the first frame and costs nothing; it only
                    // restarts anything if the host switched codec while
                    // connected.
                    Log.i("UScreen", "Host is sending $codec — rebuilding the decoder")
                    val wasRunning = !penOnlyMode
                    if (wasRunning) vr.stop()
                    vr.mimeType = mime
                    if (wasRunning) vr.start()
                }
            }
        }
        touchCapture?.onModeKnown = { penOnly ->
            runOnUiThread {
                penOnlyMode = penOnly
                if (penOnly) videoReceiver?.stop() else videoReceiver?.start()
            }
        }

        // Report the real screen size (landscape-oriented) so the host can
        // size the virtual display to match this tablet exactly.
        @Suppress("DEPRECATION")
        val size = android.graphics.Point().also {
            windowManager.defaultDisplay.getRealSize(it)
        }
        if (size.x > 0 && size.y > 0) {
            val w = maxOf(size.x, size.y)
            val h = minOf(size.x, size.y)
            // Physical size too: the host puts it in the EDID, and the desktop
            // derives its DPI — and therefore its default scale — from that.
            // Paired long-edge-to-long-edge so it matches the w/h above
            // regardless of the panel's natural orientation.
            val dm = resources.displayMetrics
            val mmA = if (dm.xdpi > 1f) size.x / dm.xdpi * 25.4f else 0f
            val mmB = if (dm.ydpi > 1f) size.y / dm.ydpi * 25.4f else 0f
            val wMm = maxOf(mmA, mmB).roundToInt()
            val hMm = minOf(mmA, mmB).roundToInt()
            touchCapture?.setNativeResolution(w, h, wMm, hMm)
            videoReceiver?.formatWidth = w
            videoReceiver?.formatHeight = h
        }

        setContent {
            UScreenTheme {
                UScreenMain(
                    penOnly = penOnlyMode,
                    updateAvailable = updateAvailable,
                    videoReceiver = videoReceiver,
                    touchCapture = touchCapture,
                    prefs = prefs,
                    onSurfaceReady = { surfaceView ->
                        videoReceiver?.setSurface(surfaceView)
                        touchCapture?.setSurfaceView(surfaceView)
                    },
                    onSurfaceDestroyed = { videoReceiver?.onSurfaceDestroyed() }
                )
            }
        }

        // Start foreground service to prevent Samsung from killing us
        val serviceIntent = Intent(this, StreamingService::class.java)
        startForegroundService(serviceIntent)

        // Enable fullscreen AFTER setContent so DecorView exists
        window.decorView.post {
            enableImmersiveMode()
        }
    }

    /**
     * Ask for the panel's fastest mode.
     *
     * Refresh rate is a direct latency cost, not just a smoothness one: a
     * decoded frame waits for the next vsync before it is visible, so 60Hz adds
     * up to 16.7ms (~8ms on average) versus 8.3ms (~4ms) at 120Hz. Measured
     * decode+render was ~14.8ms with the panel at 60Hz.
     *
     * This can only ask. Samsung's "Motion smoothness: Standard" setting
     * (`secure refresh_rate_mode = 0`) caps the panel at 60Hz system-wide and
     * overrides any app request — switching it to Adaptive is the user's call,
     * and is worth more latency than any change on the host side.
     */
    private fun requestHighestRefreshRate() {
        try {
            @Suppress("DEPRECATION")
            val disp = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) display
                else windowManager.defaultDisplay
            val best = disp?.supportedModes?.maxByOrNull { it.refreshRate } ?: return

            // Reassigning the same LayoutParams instance can be ignored, so
            // apply the change through an explicit set.
            val lp = window.attributes
            lp.preferredDisplayModeId = best.modeId
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                lp.preferredRefreshRate = best.refreshRate
            }
            window.attributes = lp

            android.util.Log.i(
                "UScreen",
                "Requested display mode ${best.modeId} @ ${best.refreshRate}Hz " +
                    "(current ${disp.refreshRate}Hz)"
            )
        } catch (e: Exception) {
            android.util.Log.w("UScreen", "Could not request a refresh rate: ${e.message}")
        }
    }

    private fun enableImmersiveMode() {
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                window.setDecorFitsSystemWindows(false)
                window.insetsController?.let { controller ->
                    controller.hide(
                        android.view.WindowInsets.Type.statusBars() or
                        android.view.WindowInsets.Type.navigationBars()
                    )
                    controller.systemBarsBehavior =
                        WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
                }
            } else {
                @Suppress("DEPRECATION")
                window.decorView.systemUiVisibility = (
                    View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY or
                    View.SYSTEM_UI_FLAG_FULLSCREEN or
                    View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or
                    View.SYSTEM_UI_FLAG_LAYOUT_STABLE or
                    View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION or
                    View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                )
            }
        } catch (e: Exception) {
            // Fallback: some Samsung firmwares have issues with insetsController
            android.util.Log.w("UScreen", "Immersive mode failed: ${e.message}")
        }
    }

    /**
     * Stylus hover, caught at the activity rather than on the SurfaceView.
     *
     * A hover listener on the SurfaceView does not reliably receive stylus
     * hover once Compose is in the picture — anything drawn over the surface
     * sits between the pointer and that listener. Hover is what moves the
     * cursor on the host, so without it there is no way to see where the pen
     * is pointing before it touches down, which matters most in graphics
     * tablet mode where the host's screen is all you are looking at.
     *
     * Safe to take at this level precisely because hover is not a click: it
     * cannot steal taps from the settings button the way intercepting touch
     * would.
     */
    /**
     * The daemon launches us with `am start --es token <hex>`; because the
     * activity is singleTask, a running app receives that here rather than
     * being recreated. A changed token means a new daemon run, so both
     * connections are torn down and rebuilt with it.
     */
    override fun onNewIntent(intent: android.content.Intent?) {
        super.onNewIntent(intent)
        intent?.let { applyToken(it, restart = true) }
    }

    private fun applyToken(intent: android.content.Intent, restart: Boolean) {
        val fromIntent = intent.getStringExtra("token")
        if (fromIntent != null) prefs.hostToken = fromIntent
        val token = prefs.hostToken ?: return
        val changed = touchCapture?.token != token
        touchCapture?.token = token
        videoReceiver?.token = token
        // Only rebuild live connections. If we are in the background, onStart
        // will connect with the new token anyway; reconnecting here as well
        // would leave a second socket behind.
        if (restart && changed && touchCapture?.isControlConnected() == true) {
            Log.i("UScreen", "New session token — reconnecting")
            touchCapture?.disconnect()
            touchCapture?.connect()
            if (!penOnlyMode) {
                videoReceiver?.stop()
                videoReceiver?.start()
            }
        }
    }

    override fun onGenericMotionEvent(event: android.view.MotionEvent): Boolean {
        val w = window.decorView.width
        val h = window.decorView.height
        if (w > 0 && h > 0 && touchCapture?.handleHoverEvent(event, w, h) == true) {
            return true
        }
        return super.onGenericMotionEvent(event)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) {
            enableImmersiveMode()
            // Re-assert once the window is actually attached: a request made in
            // onCreate can be dropped before the window exists.
            requestHighestRefreshRate()
        }
    }

    override fun onStart() {
        super.onStart()
        // One check per process, when the app comes to the front. It is a
        // single small request and the answer changes about once a month.
        if (!updateChecked) {
            updateChecked = true
            val cur = try { packageManager.getPackageInfo(packageName, 0).versionName ?: "0" } catch (_: Exception) { "0" }
            Thread {
                val found = UpdateCheck.newerThan(cur)
                if (found != null) runOnUiThread { updateAvailable = found }
            }.start()
        }
        // The video receiver is started only once the host says it is actually
        // sending a display. In pen-only mode there is nothing to receive and
        // spinning up a decoder would waste power for no picture.
        touchCapture?.connect()
        // Only re-assert settings the user actually chose here. Pushing the
        // tablet's defaults on every start would silently overwrite whatever
        // was configured in the desktop GUI.
        if (prefs.hasUserSettings) {
            touchCapture?.sendConfig(prefs.bitrateKbps, prefs.fps)
        }
    }

    override fun onStop() {
        super.onStop()
        videoReceiver?.stop()
        touchCapture?.disconnect()
    }

    override fun onDestroy() {
        super.onDestroy()
        stopService(Intent(this, StreamingService::class.java))
    }
}

private val Accent = Color(0xFF6C63FF)
private val AccentSoft = Color(0xFF8B85FF)
private val Ok = Color(0xFF4CAF50)
private val Warn = Color(0xFFFF9800)

@Composable
fun UScreenTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = Accent,
            secondary = Color(0xFF03DAC6),
            background = Color(0xFF0A0A0A),
            surface = Color(0xFF16161F),
            surfaceVariant = Color(0xFF20202C),
        )
    ) {
        content()
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun UScreenMain(
    onSurfaceReady: (SurfaceView) -> Unit,
    penOnly: Boolean = false,
    updateAvailable: String? = null,
    onSurfaceDestroyed: () -> Unit = {},
    videoReceiver: VideoReceiver? = null,
    touchCapture: TouchCapture? = null,
    prefs: Prefs? = null,
) {
    var isConnected by remember { mutableStateOf(false) }
    var fps by remember { mutableStateOf(0f) }
    var mbps by remember { mutableStateOf(0f) }
    var showOverlay by remember { mutableStateOf(true) }
    var showSettings by remember { mutableStateOf(false) }
    var showStats by remember { mutableStateOf(prefs?.showStats ?: false) }

    val context = LocalContext.current

    LaunchedEffect(videoReceiver) {
        videoReceiver?.onConnected = {
            (context as? ComponentActivity)?.runOnUiThread { isConnected = true }
        }
        videoReceiver?.onDisconnected = {
            (context as? ComponentActivity)?.runOnUiThread { isConnected = false }
        }
    }

    // Auto-hide overlay shortly after connection
    LaunchedEffect(isConnected) {
        if (isConnected) {
            delay(3000)
            showOverlay = false
        } else {
            showOverlay = true
        }
    }

    LaunchedEffect(isConnected, showStats) {
        while (isConnected) {
            delay(1000)
            fps = videoReceiver?.getFps() ?: 0f
            mbps = videoReceiver?.getMbps() ?: 0f
        }
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Color.Black)
    ) {
        // Video surface — fills entire screen
        AndroidView(
            factory = { ctx ->
                SurfaceView(ctx).apply {
                    holder.setFormat(android.graphics.PixelFormat.OPAQUE)
                    holder.addCallback(
                        object : android.view.SurfaceHolder.Callback {
                            override fun surfaceCreated(holder: android.view.SurfaceHolder) {
                                onSurfaceReady(this@apply)
                            }
                            override fun surfaceChanged(
                                holder: android.view.SurfaceHolder,
                                format: Int, width: Int, height: Int
                            ) {
                                onSurfaceReady(this@apply)
                            }
                            override fun surfaceDestroyed(holder: android.view.SurfaceHolder) {
                                onSurfaceDestroyed()
                            }
                        }
                    )
                }
            },
            modifier = Modifier.fillMaxSize()
        )

        // Pen-only: there is no picture coming, so say so instead of leaving
        // the user staring at a "waiting for the host" spinner forever.
        AnimatedVisibility(
            visible = penOnly,
            enter = fadeIn(),
            exit = fadeOut(),
            modifier = Modifier.fillMaxSize()
        ) {
            PenOnlyScreen()
        }

        // Connection screen
        AnimatedVisibility(
            visible = !isConnected && !penOnly,
            enter = fadeIn(),
            exit = fadeOut(),
            modifier = Modifier.fillMaxSize()
        ) {
            ConnectionScreen()
        }

        // Stats chip (top-left, only while streaming)
        if (isConnected && showStats) {
            Surface(
                color = Color(0x99000000),
                shape = RoundedCornerShape(8.dp),
                modifier = Modifier
                    .align(Alignment.TopStart)
                    .padding(12.dp)
            ) {
                Text(
                    text = "%.0f fps   %.1f Mbps".format(fps, mbps),
                    fontSize = 12.sp,
                    color = Color(0xFFB0B0C0),
                    modifier = Modifier.padding(horizontal = 10.dp, vertical = 5.dp)
                )
            }
        }

        // Subtle settings handle (top-right). Sits above the video surface, so
        // taps here are NOT forwarded to the Linux host.
        Box(
            modifier = Modifier
                .align(Alignment.TopEnd)
                .padding(10.dp)
                .size(38.dp)
                .alpha(if (isConnected) 0.35f else 0.9f)
                .clip(CircleShape)
                .background(Color(0xAA20202C))
                .clickable { showSettings = true },
            contentAlignment = Alignment.Center
        ) {
            Text("⚙", fontSize = 18.sp, color = Color.White)
        }

        // "Update available" pill under the settings handle. Small, and gone
        // the moment there is nothing to say.
        if (updateAvailable != null) {
            Box(
                modifier = Modifier
                    .align(Alignment.TopEnd)
                    .padding(top = 56.dp, end = 10.dp)
                    .clip(RoundedCornerShape(14.dp))
                    .background(Color(0xCC20202C))
                    .clickable {
                        context.startActivity(
                            android.content.Intent(
                                android.content.Intent.ACTION_VIEW,
                                android.net.Uri.parse(UpdateCheck.RELEASES_PAGE)
                            )
                        )
                    }
                    .padding(horizontal = 10.dp, vertical = 6.dp)
            ) {
                Text("Update $updateAvailable available", fontSize = 12.sp, color = Color.White)
            }
        }

        if (showSettings) {
            SettingsSheet(
                prefs = prefs,
                updateAvailable = updateAvailable,
                onOpenUpdate = {
                    context.startActivity(
                        android.content.Intent(
                            android.content.Intent.ACTION_VIEW,
                            android.net.Uri.parse(UpdateCheck.RELEASES_PAGE)
                        )
                    )
                },
                penOnly = penOnly,
                onPenOnlyChange = { wantPenOnly ->
                    // Fire and forget: the host answers with the mode it
                    // actually switched to, and that answer is what moves the
                    // UI. Flipping it here as well would show the new mode
                    // even when the host never got the message.
                    touchCapture?.sendMode(wantPenOnly)
                    showSettings = false
                },
                showStats = showStats,
                onShowStatsChange = {
                    showStats = it
                    prefs?.showStats = it
                },
                onApply = { bitrateKbps, newFps ->
                    prefs?.bitrateKbps = bitrateKbps
                    prefs?.fps = newFps
                    // From now on the tablet re-asserts these on every connect.
                    prefs?.hasUserSettings = true
                    touchCapture?.sendConfig(bitrateKbps, newFps)
                },
                onDismiss = { showSettings = false }
            )
        }
    }
}

@Composable
private fun PenOnlyScreen() {
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(
                Brush.verticalGradient(
                    listOf(Color(0xFF0D0D14), Color(0xFF141B2A), Color(0xFF0D0D14))
                )
            ),
        contentAlignment = Alignment.Center
    ) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text("Graphics tablet", fontSize = 34.sp, fontWeight = FontWeight.Bold,
                color = Color.White)
            Spacer(Modifier.height(10.dp))
            Text(
                "Draw here — it goes to the screen on your computer.",
                fontSize = 15.sp, color = AccentSoft, textAlign = TextAlign.Center
            )
            Spacer(Modifier.height(28.dp))
            Text(
                "Nothing is streamed to this screen in this mode, so there is no\n" +
                    "display latency at all. Pressure, tilt and the eraser all work.",
                fontSize = 13.sp, lineHeight = 22.sp, color = Color(0xFF9A9AAE),
                textAlign = TextAlign.Center
            )
        }
    }
}

@Composable
private fun ConnectionScreen() {
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(
                Brush.verticalGradient(
                    listOf(Color(0xFF0D0D14), Color(0xFF14142A), Color(0xFF0D0D14))
                )
            ),
        contentAlignment = Alignment.Center
    ) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(
                text = "UScreen",
                fontSize = 42.sp,
                fontWeight = FontWeight.Bold,
                color = Color.White
            )
            Text(
                text = "USB second display",
                fontSize = 15.sp,
                color = AccentSoft
            )
            Spacer(modifier = Modifier.height(36.dp))
            CircularProgressIndicator(
                color = Accent,
                strokeWidth = 3.dp,
                modifier = Modifier.size(40.dp)
            )
            Spacer(modifier = Modifier.height(36.dp))
            Card(
                shape = RoundedCornerShape(16.dp),
                colors = CardDefaults.cardColors(containerColor = Color(0x8C1A1A2A))
            ) {
                Column(
                    modifier = Modifier.padding(horizontal = 28.dp, vertical = 20.dp),
                    horizontalAlignment = Alignment.CenterHorizontally
                ) {
                    Text(
                        text = "Waiting for the host…",
                        fontSize = 16.sp,
                        color = Warn,
                        fontWeight = FontWeight.Medium
                    )
                    Spacer(modifier = Modifier.height(10.dp))
                    Text(
                        text = "1. Connect the USB cable\n" +
                            "2. Allow USB debugging if asked\n" +
                            "3. Make sure uscreen is running on your PC",
                        fontSize = 13.sp,
                        lineHeight = 22.sp,
                        color = Color(0xFF9A9AAE),
                        textAlign = TextAlign.Start
                    )
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SettingsSheet(
    prefs: Prefs?,
    updateAvailable: String?,
    onOpenUpdate: () -> Unit,
    penOnly: Boolean,
    onPenOnlyChange: (Boolean) -> Unit,
    showStats: Boolean,
    onShowStatsChange: (Boolean) -> Unit,
    onApply: (bitrateKbps: Int, fps: Int) -> Unit,
    onDismiss: () -> Unit,
) {
    var bitrateMbps by remember {
        mutableStateOf((prefs?.bitrateKbps ?: Prefs.DEFAULT_BITRATE_KBPS) / 1000f)
    }
    var fpsChoice by remember { mutableStateOf(prefs?.fps ?: Prefs.DEFAULT_FPS) }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        containerColor = Color(0xFF16161F)
    ) {
        Column(modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp)) {
            Text(
                "Settings",
                fontSize = 20.sp,
                fontWeight = FontWeight.Bold,
                color = Color.White
            )
            Spacer(Modifier.height(20.dp))

            if (updateAvailable != null) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(10.dp))
                        .background(Color(0x3350507A))
                        .clickable { onOpenUpdate() }
                        .padding(12.dp)
                ) {
                    Column(modifier = Modifier.weight(1f)) {
                        Text("Update available: $updateAvailable", fontSize = 14.sp, color = Color.White)
                        Text(
                            "Tap to open the release page. Update the desktop side too — they ship together.",
                            fontSize = 11.sp,
                            color = Color(0xFF9A9AB0)
                        )
                    }
                }
                Spacer(Modifier.height(16.dp))
            }

            // What the tablet is for, right now. Everything below only applies
            // when it is a screen, so the stream controls fold away when it
            // isn't.
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text("Graphics tablet", fontSize = 14.sp, color = Color(0xFFB0B0C0))
                    Text(
                        "Draw on the computer's own screen with the pen, " +
                            "instead of showing a second screen here",
                        fontSize = 11.sp,
                        color = Color(0xFF6A6A7E)
                    )
                }
                Spacer(Modifier.width(12.dp))
                Switch(
                    checked = penOnly,
                    onCheckedChange = onPenOnlyChange,
                    colors = SwitchDefaults.colors(checkedTrackColor = Accent)
                )
            }
            Spacer(Modifier.height(20.dp))

            if (!penOnly) {
            Text(
                "Bitrate: ${bitrateMbps.roundToInt()} Mbps",
                fontSize = 14.sp,
                color = Color(0xFFB0B0C0)
            )
            Slider(
                value = bitrateMbps,
                onValueChange = { bitrateMbps = it },
                valueRange = (Prefs.MIN_BITRATE_KBPS / 1000).toFloat()..
                        (Prefs.MAX_BITRATE_KBPS / 1000).toFloat(),
                steps = 10,
                colors = SliderDefaults.colors(thumbColor = Accent, activeTrackColor = Accent)
            )
            Text(
                "20 Mbps is plenty for text and UI. Going higher does not look sharper once " +
                    "the USB link is saturated — it only adds delay.",
                fontSize = 11.sp,
                color = Color(0xFF6A6A7E)
            )
            Spacer(Modifier.height(20.dp))

            Text("Frame rate", fontSize = 14.sp, color = Color(0xFFB0B0C0))
            Spacer(Modifier.height(8.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                // 120 is not offered: the generated EDID caps the virtual mode
                // at 90 Hz, so anything above would be duplicate frames.
                listOf(30, 60, 90).forEach { f ->
                    FilterChip(
                        selected = fpsChoice == f,
                        onClick = { fpsChoice = f },
                        label = { Text("$f fps") },
                        colors = FilterChipDefaults.filterChipColors(
                            selectedContainerColor = Accent,
                            selectedLabelColor = Color.White
                        )
                    )
                }
            }
            Spacer(Modifier.height(20.dp))
            }

            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text("Show stats overlay", fontSize = 14.sp, color = Color(0xFFB0B0C0))
                    Text(
                        "FPS and bandwidth in the corner",
                        fontSize = 11.sp,
                        color = Color(0xFF6A6A7E)
                    )
                }
                Switch(
                    checked = showStats,
                    onCheckedChange = onShowStatsChange,
                    colors = SwitchDefaults.colors(checkedTrackColor = Accent)
                )
            }
            Spacer(Modifier.height(24.dp))

            if (!penOnly) {
                Button(
                    onClick = {
                        onApply((bitrateMbps * 1000).roundToInt(), fpsChoice)
                        onDismiss()
                    },
                    modifier = Modifier.fillMaxWidth(),
                    colors = ButtonDefaults.buttonColors(containerColor = Accent)
                ) {
                    Text("Apply", fontSize = 16.sp, modifier = Modifier.padding(vertical = 4.dp))
                }
                Spacer(Modifier.height(8.dp))
                Text(
                    "Applying restarts the stream for a moment.",
                    fontSize = 11.sp,
                    color = Color(0xFF6A6A7E),
                    textAlign = TextAlign.Center,
                    modifier = Modifier.fillMaxWidth()
                )
            }
            Spacer(Modifier.height(24.dp))
        }
    }
}
