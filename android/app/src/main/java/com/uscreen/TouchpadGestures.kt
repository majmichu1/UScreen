package com.uscreen

import kotlin.math.abs
import kotlin.math.hypot

/**
 * Turns finger contacts into what a laptop touchpad does: move, click, drag,
 * right-click and scroll. It knows nothing about Android; the caller feeds
 * it contacts (an id, a position in pixels, a time in milliseconds) and gets
 * plain mouse actions back through [Output].
 *
 *  - one finger moving: pointer movement
 *  - quick tap: left click
 *  - tap, then touch again and move: drag with the left button held; touch and
 *    lift without moving is a second click (double click)
 *  - two fingers moving: scroll
 *  - quick two-finger tap: right click; three fingers: middle click
 *
 * A finger left over after a two-finger gesture does nothing until every
 * finger is up, otherwise lifting one finger would make the pointer jump.
 */
internal class TouchpadGestures(
    private val out: Output,
    /** Pointer speed: pixels of finger travel to pixels sent to the computer. */
    var gain: Float = DEFAULT_GAIN,
    /** Content follows the fingers, as on a phone. */
    var naturalScroll: Boolean = true,
) {
    interface Output {
        fun move(dx: Float, dy: Float)
        /** 0 = left, 1 = right, 2 = middle. */
        fun button(button: Int, down: Boolean)
        fun scroll(dx: Float, dy: Float)
    }

    private enum class Mode { IDLE, TAP_OR_MOVE, MOVING, DRAGGING, SCROLLING, ABSORBING }

    private class Contact(var x: Float, var y: Float, val downX: Float, val downY: Float)

    private val contacts = LinkedHashMap<Int, Contact>()
    private var mode = Mode.IDLE
    private var startTime = 0L
    private var maxFingers = 0
    private var travelled = false
    private var lastTapUp = -1L
    private var dragCandidate = false
    private var scrollStarted = false

    fun down(id: Int, x: Float, y: Float, time: Long) {
        if (contacts.isEmpty()) {
            startTime = time
            maxFingers = 0
            travelled = false
            scrollStarted = false
            dragCandidate = lastTapUp >= 0 && time - lastTapUp <= DOUBLE_TAP_MS
            mode = Mode.TAP_OR_MOVE
        }
        contacts[id] = Contact(x, y, x, y)
        maxFingers = maxOf(maxFingers, contacts.size)
        if (contacts.size >= 2) {
            // A second finger: this is a scroll or a multi-finger tap, never a
            // drag, and a pointer already in motion stops where it is.
            dragCandidate = false
            if (mode == Mode.DRAGGING) {
                out.button(0, false)
                mode = Mode.ABSORBING
            } else if (mode != Mode.ABSORBING) {
                mode = Mode.SCROLLING
            }
        }
    }

    fun move(id: Int, x: Float, y: Float) {
        val c = contacts[id] ?: return
        val dx = x - c.x
        val dy = y - c.y
        c.x = x
        c.y = y
        when (mode) {
            Mode.TAP_OR_MOVE -> {
                if (hypot(x - c.downX, y - c.downY) > SLOP_PX) {
                    travelled = true
                    // Everything since the finger landed, so nothing is lost
                    // to the dead zone.
                    val ox = x - c.downX
                    val oy = y - c.downY
                    if (dragCandidate) {
                        out.button(0, true)
                        mode = Mode.DRAGGING
                    } else {
                        mode = Mode.MOVING
                    }
                    out.move(ox * gain, oy * gain)
                }
            }
            Mode.MOVING, Mode.DRAGGING -> out.move(dx * gain, dy * gain)
            Mode.SCROLLING -> {
                if (contacts.size < 2) return
                if (!scrollStarted) {
                    val far = contacts.values.any { hypot(it.x - it.downX, it.y - it.downY) > SLOP_PX }
                    if (!far) return
                    scrollStarted = true
                    travelled = true
                }
                // The average of the fingers' travel, shared out per event so
                // two fingers moving together scroll at one finger's speed.
                val sx = dx / contacts.size
                val sy = dy / contacts.size
                val sign = if (naturalScroll) 1f else -1f
                out.scroll(sx * sign, sy * sign)
            }
            else -> {}
        }
    }

    fun up(id: Int, time: Long) {
        if (contacts.remove(id) == null) return
        if (contacts.isNotEmpty()) {
            if (mode == Mode.SCROLLING || mode == Mode.MOVING) mode = Mode.ABSORBING
            return
        }

        val quick = time - startTime <= TAP_MS
        when (mode) {
            Mode.TAP_OR_MOVE, Mode.SCROLLING, Mode.ABSORBING -> {
                if (!travelled && quick) {
                    when (maxFingers) {
                        1 -> {
                            click(0)
                            lastTapUp = time
                        }
                        2 -> click(1)
                        3 -> click(2)
                    }
                    if (maxFingers != 1) lastTapUp = -1
                } else {
                    lastTapUp = -1
                }
            }
            Mode.DRAGGING -> {
                out.button(0, false)
                lastTapUp = -1
            }
            else -> lastTapUp = -1
        }
        mode = Mode.IDLE
    }

    /** The system took the touch away (a gesture, a dialog, the pen). */
    fun cancel() {
        if (mode == Mode.DRAGGING) out.button(0, false)
        contacts.clear()
        mode = Mode.IDLE
        lastTapUp = -1
    }

    private fun click(button: Int) {
        out.button(button, true)
        out.button(button, false)
    }

    companion object {
        /** Travel before a touch counts as movement and not a tap. */
        const val DEFAULT_GAIN = 1.6f
        const val SLOP_PX = 12f
        const val TAP_MS = 250L
        const val DOUBLE_TAP_MS = 300L
    }
}
