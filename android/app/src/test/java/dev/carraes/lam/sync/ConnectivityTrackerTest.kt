package dev.carraes.lam.sync

import org.junit.Assert.*
import org.junit.Test

class ConnectivityTrackerTest {
    @Test fun `unobserved startup network cannot keep VPN online after the last observed network is lost`() {
        val tracker = ConnectivityTracker(null, false)
        // A startup network excluded by the passive request delivers no callback and cannot
        // enter membership. There is deliberately no physical-snapshot seeding API.
        tracker.seedDefault("vpn", true, true)
        assertFalse(tracker.state.value.available)
        tracker.physicalAvailable("observed-wifi")
        assertTrue(tracker.state.value.available)
        tracker.physicalLost("observed-wifi")
        assertFalse(tracker.state.value.available)
    }

    @Test fun `default snapshot does not overwrite newer callbacks`() {
        val tracker = ConnectivityTracker(null, false)
        tracker.available("mobile")
        tracker.capabilities("mobile", true)
        tracker.seedDefault("old-wifi", false, false)
        assertTrue(tracker.state.value.available)
        tracker.lost("mobile")
        assertFalse(tracker.state.value.available)
    }

    @Test fun `default snapshot provides startup connectivity before callback delivery`() {
        val tracker = ConnectivityTracker(null, false)
        tracker.seedDefault("wifi", true, false)
        assertTrue(tracker.state.value.available)
    }

    @Test fun `VPN-only default needs a physical internet network but not its validation`() {
        val tracker = ConnectivityTracker("vpn", true, initialVpn = true)
        assertFalse(tracker.state.value.available)
        tracker.physicalAvailable("wifi-without-validation")
        assertTrue(tracker.state.value.available)
        val online = tracker.state.value
        tracker.physicalLost("wifi-without-validation")
        assertFalse(tracker.state.value.available)
        tracker.physicalAvailable("wifi-without-validation")
        assertTrue(tracker.state.value.available)
        assertNotEquals(online.epoch, tracker.state.value.epoch)
    }

    @Test fun `unrelated and non-last physical losses do not invalidate VPN`() {
        val tracker = ConnectivityTracker("vpn", true, initialVpn = true)
        tracker.physicalAvailable("wifi")
        tracker.physicalAvailable("mobile")
        val online = tracker.state.value
        tracker.physicalLost("unrelated")
        tracker.physicalLost("wifi")
        assertEquals(online, tracker.state.value)
        tracker.physicalLost("mobile")
        assertFalse(tracker.state.value.available)
    }

    @Test fun `physical callbacks delivered before default startup snapshot settle VPN availability`() {
        val tracker = ConnectivityTracker(null, false)
        tracker.physicalAvailable("mobile")
        assertFalse(tracker.state.value.available)
        tracker.seedDefault("vpn", true, true)
        assertTrue(tracker.state.value.available)
        tracker.physicalLost("mobile")
        assertFalse(tracker.state.value.available)
    }

    @Test fun `a replacement network needs validation and old loss callbacks cannot invalidate it`() {
        val tracker = ConnectivityTracker("wifi", true)
        val original = tracker.state.value.epoch
        tracker.available("mobile")
        assertFalse(tracker.state.value.available)
        assertNotEquals(original, tracker.state.value.epoch)
        tracker.capabilities("wifi", true)
        assertFalse(tracker.state.value.available)
        tracker.capabilities("mobile", true)
        assertTrue(tracker.state.value.available)
        val replacement = tracker.state.value
        tracker.lost("wifi")
        assertEquals(replacement, tracker.state.value)
        tracker.lost("mobile")
        assertFalse(tracker.state.value.available)
        assertNotEquals(replacement.epoch, tracker.state.value.epoch)
    }

    @Test fun `duplicate availability does not interrupt current reconciliation but validation loss does`() {
        val tracker = ConnectivityTracker("wifi", true)
        tracker.available("wifi")
        tracker.capabilities("wifi", true)
        assertEquals(ConnectivityStatus(true, 0), tracker.state.value)
        tracker.capabilities("wifi", false)
        assertEquals(ConnectivityStatus(false, 1), tracker.state.value)
        tracker.capabilities("wifi", true)
        assertEquals(ConnectivityStatus(true, 2), tracker.state.value)
    }
}
