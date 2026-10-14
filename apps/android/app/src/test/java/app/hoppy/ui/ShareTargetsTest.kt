package app.hoppy.ui

import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class ShareTargetsTest {
    private val uuid = "5f0c2b7e-1d2a-4c3b-9e8f-0a1b2c3d4e5f"

    @Test
    fun shortcutIdRoundTrip() {
        assertEquals("device:$uuid", ShareTargets.deviceShortcutId(uuid))
        assertEquals(uuid, ShareTargets.peerIdOf(ShareTargets.deviceShortcutId(uuid)))
    }

    @Test
    fun nonDeviceOrInvalidIdsAreIgnored() {
        assertNull(ShareTargets.peerIdOf(null))
        assertNull(ShareTargets.peerIdOf(ShareTargets.NEARBY_ID))
        assertNull(ShareTargets.peerIdOf("device:"))
        assertNull(ShareTargets.peerIdOf("device:a\tb"))
        assertNull(ShareTargets.peerIdOf("device:../x/y"))
        assertNull(ShareTargets.peerIdOf("device:" + "a".repeat(65)))
    }

    @Test
    fun bumpPutsMostRecentFirstAndEvicts() {
        var list = emptyList<RecentTarget>()
        for (i in 1..5) list = ShareTargets.bump(list, RecentTarget("id$i", "Dev $i"), max = 4).recent
        assertEquals(listOf("id5", "id4", "id3", "id2"), list.map { it.peerId })

        val again = ShareTargets.bump(list, RecentTarget("id3", "Renamed"), max = 4)
        assertEquals(listOf("id3", "id5", "id4", "id2"), again.recent.map { it.peerId })
        assertEquals("Renamed", again.recent.first().name)
        assertTrue(again.evicted.isEmpty())

        val evicting = ShareTargets.bump(again.recent, RecentTarget("id9", "New"), max = 4)
        assertEquals(listOf("id2"), evicting.evicted.map { it.peerId })
    }

    @Test
    fun encodeDecodeRoundTripCleansNames() {
        val list = listOf(RecentTarget(uuid, "Lap\ttop\n"), RecentTarget("b", "  "))
        val decoded = ShareTargets.decode(ShareTargets.encode(list))
        assertEquals(listOf(RecentTarget(uuid, "Laptop"), RecentTarget("b", "?")), decoded)
    }

    @Test
    fun decodeSkipsGarbage() {
        assertEquals(emptyList<RecentTarget>(), ShareTargets.decode(null))
        assertEquals(emptyList<RecentTarget>(), ShareTargets.decode(""))
        val decoded = ShareTargets.decode("no-tab\n\tname\nbad id\tx\nok\tOne\nok\tDup")
        assertEquals(listOf(RecentTarget("ok", "One")), decoded)
    }
}
