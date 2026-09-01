package com.uscreen

import android.content.Context
import android.content.SharedPreferences

/** Persisted user settings on the tablet side. */
class Prefs(context: Context) {
    private val sp: SharedPreferences =
        context.getSharedPreferences("uscreen", Context.MODE_PRIVATE)

    companion object {
        /**
         * Defaults deliberately match the host's own defaults. They used to be
         * 200 Mbps / 90 fps, which the app pushed to the host on every start —
         * overwriting whatever was configured on the desktop and driving the
         * encoder far past what the USB link carries, so frames queued up and
         * latency grew without bound.
         */
        const val DEFAULT_BITRATE_KBPS = 20_000
        const val DEFAULT_FPS = 60

        /** Kept in sync with `config::MAX_BITRATE_KBPS` on the host. */
        const val MAX_BITRATE_KBPS = 60_000
        const val MIN_BITRATE_KBPS = 5_000
    }

    var bitrateKbps: Int
        get() = sp.getInt("bitrate_kbps", DEFAULT_BITRATE_KBPS)
            .coerceIn(MIN_BITRATE_KBPS, MAX_BITRATE_KBPS)
        set(v) = sp.edit().putInt("bitrate_kbps", v).apply()

    var fps: Int
        get() = sp.getInt("fps", DEFAULT_FPS)
        set(v) = sp.edit().putInt("fps", v).apply()

    var showStats: Boolean
        get() = sp.getBoolean("show_stats", false)
        set(v) = sp.edit().putBoolean("show_stats", v).apply()

    /**
     * True once the user has actually applied settings from the sheet.
     *
     * Until then the tablet stays silent instead of pushing its defaults on
     * every connect: the desktop GUI is the source of truth, and a tablet that
     * announces stale defaults at startup silently undoes whatever was set
     * there. Only a deliberate "Apply" gives the tablet the right to speak.
     */
    var hasUserSettings: Boolean
        get() = sp.getBoolean("has_user_settings", false)
        set(v) = sp.edit().putBoolean("has_user_settings", v).apply()

    /**
     * The daemon's session token, delivered as an intent extra when it
     * launches the app over adb. Kept so a relaunch by hand within the same
     * daemon run still authenticates; a new daemon run hands out a new one.
     */
    var hostToken: String?
        get() = sp.getString("host_token", null)
        set(v) = sp.edit().putString("host_token", v).apply()

    /** Shown once, after the first time video actually arrived. */
    var thankedOnce: Boolean
        get() = sp.getBoolean("thanked_once", false)
        set(v) = sp.edit().putBoolean("thanked_once", v).apply()
}
