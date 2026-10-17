package com.uscreen

import android.view.KeyEvent

/** Map the observed Focus Pen Pro squeeze key to an independent stylus-button hold. */
internal class XiaomiPenSupport(
    private val setSqueeze: (Boolean) -> Unit,
) {
    private var resumed = false

    fun resume() {
        resumed = true
    }

    fun pause() {
        resumed = false
        setSqueeze(false)
    }

    fun handleKeyEvent(event: KeyEvent): Boolean {
        if (!resumed || event.keyCode != SQUEEZE_KEY) return false
        // 194 is also KEYCODE_BUTTON_7. Do not consume gamepad/keyboard keys.
        val device = event.device ?: return false
        val focusPen = device.vendorId == 0x0022 && device.productId == 0x5081 &&
            device.name.contains("Focus Pen Pro", ignoreCase = true)
        if (!focusPen) return false
        when (event.action) {
            KeyEvent.ACTION_DOWN -> {
                if (event.isCanceled) setSqueeze(false)
                else if (event.repeatCount == 0) setSqueeze(true)
            }
            KeyEvent.ACTION_UP -> setSqueeze(false)
            else -> return false
        }
        return true
    }

    companion object {
        private const val SQUEEZE_KEY = 194
    }
}
