package com.uscreen

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.os.Build
import android.os.IBinder
import android.os.PowerManager

class StreamingService : Service() {
    companion object {
        const val CHANNEL_ID = "uscreen_streaming"
        const val NOTIFICATION_ID = 1
    }

    private var wakeLock: PowerManager.WakeLock? = null

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val notificationIntent = Intent(this, MainActivity::class.java)
        val pendingIntent = PendingIntent.getActivity(
            this, 0, notificationIntent,
            PendingIntent.FLAG_IMMUTABLE
        )

        val notification = Notification.Builder(this, CHANNEL_ID)
            .setContentTitle("UScreen Active")
            .setContentText("Streaming display to this device")
            .setSmallIcon(android.R.drawable.ic_menu_view)
            .setContentIntent(pendingIntent)
            .setOngoing(true)
            .build()

        // Never let this kill the app. The foreground service only helps the
        // process survive backgrounding — streaming itself does not depend on
        // it, so a platform rejection (the FGS type rules change between
        // Android releases) must degrade, not crash.
        try {
            startForeground(NOTIFICATION_ID, notification)
        } catch (e: Exception) {
            android.util.Log.e(
                "UScreenService",
                "startForeground rejected: ${e.message}. Continuing without it.",
                e
            )
            stopSelf()
            return START_NOT_STICKY
        }

        // Acquire a partial wake lock to prevent CPU sleep.
        //
        // Guarded against re-entry: onStartCommand runs again on every
        // startService call and on every START_STICKY restart, and the previous
        // version replaced the field each time without releasing the old lock,
        // leaking one wake lock per restart.
        if (wakeLock?.isHeld != true) {
            val pm = getSystemService(POWER_SERVICE) as PowerManager
            wakeLock = pm.newWakeLock(
                PowerManager.PARTIAL_WAKE_LOCK,
                "UScreen::StreamingWakeLock"
            ).apply {
                setReferenceCounted(false)
                acquire(4 * 60 * 60 * 1000L) // 4 hours max
            }
        }

        return START_STICKY
    }

    override fun onDestroy() {
        wakeLock?.let {
            if (it.isHeld) it.release()
        }
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "UScreen Streaming",
                NotificationManager.IMPORTANCE_LOW
            ).apply {
                description = "Keeps UScreen alive while streaming"
                setShowBadge(false)
            }
            val nm = getSystemService(NotificationManager::class.java)
            nm.createNotificationChannel(channel)
        }
    }
}
