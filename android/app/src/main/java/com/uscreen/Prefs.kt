package com.uscreen

import android.content.Context
import android.content.SharedPreferences

/** Persisted user settings on the tablet side. */
class Prefs(context: Context) {
    private val sp: SharedPreferences =
        context.getSharedPreferences("uscreen", Context.MODE_PRIVATE)

    var bitrateKbps: Int
        get() = sp.getInt("bitrate_kbps", 20000)
        set(v) = sp.edit().putInt("bitrate_kbps", v).apply()

    var fps: Int
        get() = sp.getInt("fps", 60)
        set(v) = sp.edit().putInt("fps", v).apply()

    var showStats: Boolean
        get() = sp.getBoolean("show_stats", false)
        set(v) = sp.edit().putBoolean("show_stats", v).apply()
}
