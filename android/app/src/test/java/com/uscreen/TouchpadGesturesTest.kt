package com.uscreen

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class TouchpadGesturesTest {
    private val log = mutableListOf<String>()
    private val out = object : TouchpadGestures.Output {
        override fun move(dx: Float, dy: Float) { log += "move ${dx.toInt()},${dy.toInt()}" }
        override fun button(button: Int, down: Boolean) { log += "button $button ${if (down) "down" else "up"}" }
        override fun scroll(dx: Float, dy: Float) { log += "scroll ${dx.toInt()},${dy.toInt()}" }
    }
    private fun pad(gain: Float = 1f, natural: Boolean = true) =
        TouchpadGestures(out, gain, natural)

    @Test fun quickTapIsALeftClick() {
        val g = pad()
        g.down(0, 100f, 100f, 0); g.up(0, 100)
        assertEquals(listOf("button 0 down", "button 0 up"), log)
    }

    @Test fun slowTouchIsNotAClick() {
        val g = pad()
        g.down(0, 100f, 100f, 0); g.up(0, 600)
        assertTrue(log.isEmpty())
    }

    @Test fun movingPastTheDeadZoneSendsEverythingSinceTouchdown() {
        val g = pad()
        g.down(0, 100f, 100f, 0)
        g.move(0, 105f, 100f)          // inside the dead zone
        g.move(0, 120f, 100f)          // out: 20 px since touchdown
        g.move(0, 130f, 105f)
        g.up(0, 40)
        assertEquals(listOf("move 20,0", "move 10,5"), log)
    }

    @Test fun gainScalesMovement() {
        val g = pad(gain = 2f)
        g.down(0, 0f, 0f, 0); g.move(0, 20f, 0f); g.move(0, 30f, 0f)
        assertEquals(listOf("move 40,0", "move 20,0"), log)
    }

    @Test fun tapThenTouchAndMoveDrags() {
        val g = pad()
        g.down(0, 50f, 50f, 0); g.up(0, 80)           // click
        log.clear()
        g.down(0, 50f, 50f, 200)
        g.move(0, 80f, 50f)
        g.move(0, 90f, 50f)
        g.up(0, 260)
        assertEquals(listOf("button 0 down", "move 30,0", "move 10,0", "button 0 up"), log)
    }

    @Test fun tapThenTapAgainIsTwoClicks() {
        val g = pad()
        g.down(0, 50f, 50f, 0); g.up(0, 80)
        g.down(0, 50f, 50f, 200); g.up(0, 260)
        assertEquals(List(2) { listOf("button 0 down", "button 0 up") }.flatten(), log)
    }

    @Test fun aLateSecondTouchIsNotADrag() {
        val g = pad()
        g.down(0, 50f, 50f, 0); g.up(0, 80)
        log.clear()
        g.down(0, 50f, 50f, 1000)
        g.move(0, 90f, 50f)
        assertEquals(listOf("move 40,0"), log)
    }

    @Test fun twoFingersMovingScrollWithTheAverage() {
        val g = pad()
        g.down(0, 100f, 100f, 0); g.down(1, 200f, 100f, 5)
        g.move(0, 100f, 130f)
        g.move(1, 200f, 130f)
        assertEquals(listOf("scroll 0,15", "scroll 0,15"), log)
    }

    @Test fun scrollDirectionCanBeInverted() {
        val g = pad(natural = false)
        g.down(0, 100f, 100f, 0); g.down(1, 200f, 100f, 5)
        g.move(0, 100f, 130f)
        assertEquals(listOf("scroll 0,-15"), log)
    }

    @Test fun twoFingerTapIsARightClick() {
        val g = pad()
        g.down(0, 100f, 100f, 0); g.down(1, 200f, 100f, 10)
        g.up(1, 90); g.up(0, 100)
        assertEquals(listOf("button 1 down", "button 1 up"), log)
    }

    @Test fun threeFingerTapIsAMiddleClick() {
        val g = pad()
        g.down(0, 100f, 100f, 0); g.down(1, 200f, 100f, 5); g.down(2, 300f, 100f, 10)
        g.up(2, 80); g.up(1, 85); g.up(0, 90)
        assertEquals(listOf("button 2 down", "button 2 up"), log)
    }

    @Test fun theFingerLeftAfterAScrollDoesNotMoveThePointer() {
        val g = pad()
        g.down(0, 100f, 100f, 0); g.down(1, 200f, 100f, 5)
        g.move(0, 100f, 140f)
        log.clear()
        g.up(1, 40)
        g.move(0, 150f, 200f)
        g.up(0, 80)
        assertTrue(log.isEmpty())
    }

    @Test fun cancelReleasesAHeldDrag() {
        val g = pad()
        g.down(0, 0f, 0f, 0); g.up(0, 50)
        g.down(0, 0f, 0f, 100); g.move(0, 40f, 0f)
        log.clear()
        g.cancel()
        assertEquals(listOf("button 0 up"), log)
    }

    @Test fun aSecondFingerDuringADragLetsGo() {
        val g = pad()
        g.down(0, 0f, 0f, 0); g.up(0, 50)
        g.down(0, 0f, 0f, 100); g.move(0, 40f, 0f)
        log.clear()
        g.down(1, 100f, 0f, 130)
        assertEquals(listOf("button 0 up"), log)
    }
}
