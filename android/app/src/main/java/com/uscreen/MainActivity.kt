package com.uscreen

import android.content.Intent
import android.os.Build
import android.os.Bundle
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
    private var videoReceiver: VideoReceiver? = null
    private var touchCapture: TouchCapture? = null
    private lateinit var prefs: Prefs

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Keep screen on while streaming
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        prefs = Prefs(this)
        videoReceiver = VideoReceiver()
        touchCapture = TouchCapture()

        // Report the real screen size (landscape-oriented) so the host can
        // size the virtual display to match this tablet exactly.
        @Suppress("DEPRECATION")
        val size = android.graphics.Point().also {
            windowManager.defaultDisplay.getRealSize(it)
        }
        if (size.x > 0 && size.y > 0) {
            val w = maxOf(size.x, size.y)
            val h = minOf(size.x, size.y)
            touchCapture?.setNativeResolution(w, h)
            videoReceiver?.formatWidth = w
            videoReceiver?.formatHeight = h
        }

        setContent {
            UScreenTheme {
                UScreenMain(
                    videoReceiver = videoReceiver,
                    touchCapture = touchCapture,
                    prefs = prefs,
                    onSurfaceReady = { surfaceView ->
                        videoReceiver?.setSurface(surfaceView)
                        touchCapture?.setSurfaceView(surfaceView)
                    }
                )
            }
        }

        // Start foreground service to prevent Samsung from killing us
        val serviceIntent = Intent(this, StreamingService::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            startForegroundService(serviceIntent)
        } else {
            startService(serviceIntent)
        }

        // Enable fullscreen AFTER setContent so DecorView exists
        window.decorView.post {
            enableImmersiveMode()
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

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) {
            enableImmersiveMode()
        }
    }

    override fun onStart() {
        super.onStart()
        videoReceiver?.start()
        touchCapture?.connect()
        // Re-apply saved settings to the host on every (re)start
        touchCapture?.sendConfig(prefs.bitrateKbps, prefs.fps)
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
                            override fun surfaceDestroyed(holder: android.view.SurfaceHolder) {}
                        }
                    )
                }
            },
            modifier = Modifier.fillMaxSize()
        )

        // Connection screen
        AnimatedVisibility(
            visible = !isConnected,
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

        if (showSettings) {
            SettingsSheet(
                prefs = prefs,
                showStats = showStats,
                onShowStatsChange = {
                    showStats = it
                    prefs?.showStats = it
                },
                onApply = { bitrateKbps, newFps ->
                    prefs?.bitrateKbps = bitrateKbps
                    prefs?.fps = newFps
                    touchCapture?.sendConfig(bitrateKbps, newFps)
                },
                onDismiss = { showSettings = false }
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
    showStats: Boolean,
    onShowStatsChange: (Boolean) -> Unit,
    onApply: (bitrateKbps: Int, fps: Int) -> Unit,
    onDismiss: () -> Unit,
) {
    var bitrateMbps by remember { mutableStateOf((prefs?.bitrateKbps ?: 20000) / 1000f) }
    var fpsChoice by remember { mutableStateOf(prefs?.fps ?: 60) }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        containerColor = Color(0xFF16161F)
    ) {
        Column(modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp)) {
            Text(
                "Stream settings",
                fontSize = 20.sp,
                fontWeight = FontWeight.Bold,
                color = Color.White
            )
            Spacer(Modifier.height(20.dp))

            Text(
                "Bitrate: ${bitrateMbps.roundToInt()} Mbps",
                fontSize = 14.sp,
                color = Color(0xFFB0B0C0)
            )
            Slider(
                value = bitrateMbps,
                onValueChange = { bitrateMbps = it },
                valueRange = 5f..50f,
                steps = 44,
                colors = SliderDefaults.colors(thumbColor = Accent, activeTrackColor = Accent)
            )
            Text(
                "Higher = sharper image, lower = smoother on slow USB",
                fontSize = 11.sp,
                color = Color(0xFF6A6A7E)
            )
            Spacer(Modifier.height(20.dp))

            Text("Frame rate", fontSize = 14.sp, color = Color(0xFFB0B0C0))
            Spacer(Modifier.height(8.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf(30, 60).forEach { f ->
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
            Spacer(Modifier.height(24.dp))
        }
    }
}
