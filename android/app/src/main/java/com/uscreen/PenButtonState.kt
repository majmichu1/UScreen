package com.uscreen

/** Combine MotionEvent side buttons and key-based squeeze without one releasing the other. */
internal class PenButtonState(private val emit: (Boolean) -> Unit) {
    private var motionDown = false
    private var squeezeDown = false
    private var sentDown = false

    @Synchronized fun setMotion(down: Boolean) {
        motionDown = down
        sync()
    }

    @Synchronized fun setSqueeze(down: Boolean) {
        squeezeDown = down
        sync()
    }

    @Synchronized fun release() {
        motionDown = false
        squeezeDown = false
        sync()
    }

    /** Key-based squeeze has its own UP event, even during a brief hover exit. */
    @Synchronized fun leaveProximity() {
        motionDown = false
        sync()
    }

    /** A new socket has no held buttons; never replay a press from the old session. */
    @Synchronized fun reset() {
        motionDown = false
        squeezeDown = false
        sentDown = false
    }

    private fun sync() {
        val down = motionDown || squeezeDown
        if (down != sentDown) {
            sentDown = down
            emit(down)
        }
    }
}
