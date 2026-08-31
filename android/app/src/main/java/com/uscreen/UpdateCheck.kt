package com.uscreen

import android.util.Log
import okhttp3.OkHttpClient
import okhttp3.Request
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * "A newer release exists", and nothing more. Sideloaded apps cannot update
 * themselves silently — Android always asks the user — so this just answers
 * the question and hands over the release page.
 */
object UpdateCheck {
    private const val TAG = "UScreenUpdate"
    const val RELEASES_PAGE = "https://github.com/majmichu1/UScreen/releases/latest"
    private const val API = "https://api.github.com/repos/majmichu1/UScreen/releases/latest"

    private val client = OkHttpClient.Builder()
        .connectTimeout(10, TimeUnit.SECONDS)
        .readTimeout(10, TimeUnit.SECONDS)
        .build()

    private fun parse(v: String): Triple<Int, Int, Int> {
        val p = v.trim().removePrefix("v").split(".").map { it.toIntOrNull() ?: 0 }
        return Triple(p.getOrElse(0) { 0 }, p.getOrElse(1) { 0 }, p.getOrElse(2) { 0 })
    }

    fun isNewer(candidate: String, current: String): Boolean {
        val (a1, a2, a3) = parse(candidate); val (b1, b2, b3) = parse(current)
        return if (a1 != b1) a1 > b1 else if (a2 != b2) a2 > b2 else a3 > b3
    }

    /** Blocking; call off the main thread. Returns the newer version or null. */
    fun newerThan(current: String): String? {
        return try {
            val req = Request.Builder().url(API)
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "uscreen-android/$current")
                .build()
            client.newCall(req).execute().use { resp ->
                if (!resp.isSuccessful) return null
                val tag = JSONObject(resp.body?.string() ?: return null)
                    .optString("tag_name").removePrefix("v")
                if (tag.isNotEmpty() && isNewer(tag, current)) tag else null
            }
        } catch (e: Exception) {
            Log.d(TAG, "update check skipped: ${e.message}"); null
        }
    }
}
