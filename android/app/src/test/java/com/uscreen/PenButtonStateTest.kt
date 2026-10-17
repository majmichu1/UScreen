package com.uscreen

import org.junit.Assert.assertEquals
import org.junit.Test

class PenButtonStateTest {
    @Test fun hoverExitDoesNotReleaseSqueezeBeforeItsKeyUp() {
        val events = mutableListOf<Boolean>()
        val state = PenButtonState { events.add(it) }
        state.setSqueeze(true)
        repeat(3) { state.leaveProximity(); state.setMotion(false) }
        assertEquals(listOf(true), events)
        state.setSqueeze(false)
        assertEquals(listOf(true, false), events)
    }

    @Test fun hoverExitStillCleansUpMotionButtons() {
        val events = mutableListOf<Boolean>()
        val state = PenButtonState { events.add(it) }
        state.setMotion(true)
        state.leaveProximity()
        assertEquals(listOf(true, false), events)
        state.setMotion(true)
        state.setSqueeze(true)
        state.leaveProximity()
        assertEquals(listOf(true, false, true), events)
        state.setSqueeze(false)
        assertEquals(listOf(true, false, true, false), events)
    }

    @Test fun hoverDoesNotReleaseSqueezeAndReleaseIsNotAToggle() {
        val events = mutableListOf<Boolean>()
        val state = PenButtonState { events.add(it) }
        state.setSqueeze(true)
        repeat(20) { state.setMotion(false); state.setSqueeze(true) }
        assertEquals(listOf(true), events)
        state.setSqueeze(false)
        state.setSqueeze(false)
        assertEquals(listOf(true, false), events)
    }

    @Test fun eitherButtonSourceCanKeepTheButtonHeld() {
        for (squeezeFirst in listOf(true, false)) {
            val events = mutableListOf<Boolean>()
            val state = PenButtonState { events.add(it) }
            state.setMotion(true)
            state.setSqueeze(true)
            if (squeezeFirst) state.setSqueeze(false) else state.setMotion(false)
            assertEquals(listOf(true), events)
            if (squeezeFirst) state.setMotion(false) else state.setSqueeze(false)
            assertEquals(listOf(true, false), events)
        }
    }

    @Test fun focusLossReleasesBothSourcesOnce() {
        val events = mutableListOf<Boolean>()
        val state = PenButtonState { events.add(it) }
        state.setMotion(true)
        state.setSqueeze(true)
        state.release()
        state.release()
        state.setMotion(false)
        state.setSqueeze(false)
        assertEquals(listOf(true, false), events)
    }

    @Test fun newConnectionDoesNotReplayAnOldPress() {
        val events = mutableListOf<Boolean>()
        val state = PenButtonState { events.add(it) }
        state.setSqueeze(true)
        state.reset()
        state.setSqueeze(false)
        assertEquals(listOf(true), events)
        state.setSqueeze(true)
        state.setSqueeze(false)
        assertEquals(listOf(true, true, false), events)
    }
}
