package com.uscreen

import android.content.pm.ActivityInfo
import android.view.Surface

/**
 * Which landscape direction the tilt sensor asks for.
 *
 * `OrientationEventListener` reports degrees clockwise from the panel's
 * *natural* upright, and the natural orientation is portrait on most phones
 * and landscape on many tablets (a Wacom Movink, Lenovo and Fire tablets,
 * among others). The ranges below are written for a portrait panel, so a
 * naturally landscape one has to be turned into that frame first — otherwise
 * holding it normally asks for nothing, and turning it to portrait flips the
 * picture upside down.
 */
internal object OrientationPolicy {
    /** From the real display size and its current rotation. */
    fun naturallyLandscape(width: Int, height: Int, rotation: Int): Boolean {
        val quarterTurn = rotation == Surface.ROTATION_90 || rotation == Surface.ROTATION_270
        return (width > height) != quarterTurn
    }

    /**
     * The orientation to pin, or null to leave things as they are: upright,
     * near a boundary or an unknown reading must not flip the picture back
     * and forth. Only ±35° around either landscape direction acts.
     */
    fun landscapeForSensor(angle: Int, naturallyLandscape: Boolean): Int? {
        if (angle !in 0..359) return null
        val portraitAngle = if (naturallyLandscape) (angle + 270) % 360 else angle
        return when (portraitAngle) {
            in 235..305 -> ActivityInfo.SCREEN_ORIENTATION_LANDSCAPE
            in 55..125 -> ActivityInfo.SCREEN_ORIENTATION_REVERSE_LANDSCAPE
            else -> null
        }
    }
}
