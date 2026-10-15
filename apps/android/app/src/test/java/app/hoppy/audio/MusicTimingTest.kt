package app.hoppy.audio

import app.hoppy.core.Json
import org.junit.Test
import java.io.File
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

class MusicTimingTest {
    /** The repo's manifest, found from the test working directory (Gradle: apps/android/app). */
    private fun manifest(): File {
        var dir: File? = File(System.getProperty("user.dir")).absoluteFile
        while (dir != null) {
            val f = File(dir, "assets/music/manifest.json")
            if (f.isFile) return f
            dir = dir.parentFile
        }
        error("assets/music/manifest.json not found above ${System.getProperty("user.dir")}")
    }

    @Test
    fun barTableMatchesManifest() {
        @Suppress("UNCHECKED_CAST")
        val files = Json.decodeObject(manifest().readText())["files"] as List<Map<String, Any?>>
        val loops = files.filter {
            (it["resource"] as String).startsWith("music_") && it["loop"] == true
        }
        assertTrue(loops.isNotEmpty())
        val fromManifest = loops.groupBy { it["theme"] as String }
            .mapValues { (_, l) -> l.associate { it["kind"] as String to (it["bar_ms"] as Number).toDouble() } }
        assertEquals(fromManifest.keys, MusicTiming.BAR_MS.keys, "themes")
        for ((theme, kinds) in fromManifest) {
            assertEquals(kinds.keys, MusicTiming.BAR_MS.getValue(theme).keys, "kinds of $theme")
            for ((kind, ms) in kinds) {
                val ours = assertNotNull(MusicTiming.barMs(theme, kind))
                assertEquals(ms, ours, 0.001, "bar_ms of $theme/$kind")
            }
        }
        // Every Android resource the player may ask for exists in the manifest.
        val resources = files.map { it["resource"] as String }.toSet()
        for (theme in MusicTiming.BAR_MS.keys) {
            for (kind in listOf(MusicTiming.IDLE, MusicTiming.TRANSFER, MusicTiming.DONE)) {
                assertTrue("music_${theme}_$kind" in resources, "music_${theme}_$kind")
            }
        }
    }

    @Test
    fun delayGoesToTheNextBoundary() {
        assertEquals(1500L, MusicTiming.delayToNextBar(500, 2000.0))
        assertEquals(1500L, MusicTiming.delayToNextBar(4500, 2000.0)) // third bar of the loop
        assertEquals(100L, MusicTiming.delayToNextBar(1900, 2000.0))
    }

    @Test
    fun boundaryTooCloseWaitsOneMoreBar() {
        assertEquals(2000L, MusicTiming.delayToNextBar(0, 2000.0))
        assertEquals(2010L, MusicTiming.delayToNextBar(1990, 2000.0))
        assertEquals(40L, MusicTiming.delayToNextBar(1960, 2000.0))
    }

    @Test
    fun fractionalBarsDoNotDrift() {
        val bar = MusicTiming.barMs("purikura", MusicTiming.IDLE)!!
        // End of bar 7 of 8: boundary at 7 * bar ≈ 15555.554 ms.
        assertEquals(56L, MusicTiming.delayToNextBar(15500, bar))
    }

    @Test
    fun unknownBarSwitchesNow() {
        assertEquals(0L, MusicTiming.delayToNextBar(1234, 0.0))
        assertEquals(0L, MusicTiming.delayToNextBar(-1, 2000.0))
    }

    @Test
    fun fadesHitTheirEnds() {
        assertEquals(0f, MusicTiming.fadeGain(0f, 1f, 0f), 1e-6f)
        assertEquals(1f, MusicTiming.fadeGain(0f, 1f, 1f), 1e-6f)
        assertEquals(1f, MusicTiming.fadeGain(1f, 0f, 0f), 1e-6f)
        assertEquals(0f, MusicTiming.fadeGain(1f, 0f, 1f), 1e-6f)
        assertEquals(1f, MusicTiming.fadeGain(0f, 1f, 3f), 1e-6f) // clamped
    }

    @Test
    fun crossfadeKeepsPowerSteady() {
        for (i in 0..10) {
            val t = i / 10f
            val a = MusicTiming.fadeGain(0f, 1f, t)
            val b = MusicTiming.fadeGain(1f, 0f, t)
            assertEquals(1f, a * a + b * b, 1e-4f)
        }
    }
}
