package app.hoppy.update

import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class UpdateLogicTest {
    @Test
    fun comparesCoreVersions() {
        assertEquals(1, UpdateLogic.compare("v0.2.0", "0.1.9"))
        assertEquals(-1, UpdateLogic.compare("0.2.0", "0.10.0"))
        assertEquals(0, UpdateLogic.compare("v1.0.0", "1.0.0"))
        assertEquals(0, UpdateLogic.compare("1.2", "1.2.0"))
        assertEquals(1, UpdateLogic.compare("2", "1.99.99"))
        assertEquals(0, UpdateLogic.compare("1.0.0+build.5", "1.0.0"))
    }

    @Test
    fun preReleaseIsOlder() {
        assertEquals(-1, UpdateLogic.compare("1.0.0-rc.1", "1.0.0"))
        assertEquals(1, UpdateLogic.compare("1.0.0", "1.0.0-beta"))
        assertEquals(-1, UpdateLogic.compare("1.0.0-alpha", "1.0.0-alpha.1"))
        assertEquals(-1, UpdateLogic.compare("1.0.0-alpha.2", "1.0.0-alpha.10"))
        assertEquals(-1, UpdateLogic.compare("1.0.0-1", "1.0.0-alpha"))
        assertEquals(1, UpdateLogic.compare("1.0.1-rc.1", "1.0.0"))
    }

    @Test
    fun invalidVersionsAreNeverNewer() {
        assertNull(UpdateLogic.compare("latest", "1.0.0"))
        assertNull(UpdateLogic.compare("1.0.0", ""))
        assertNull(UpdateLogic.compare("1..0", "1.0.0"))
        assertNull(UpdateLogic.compare("1.0.0-", "1.0.0"))
        assertFalse(UpdateLogic.isNewer("garbage", "0.1.0"))
        assertFalse(UpdateLogic.isNewer(null, "0.1.0"))
        assertFalse(UpdateLogic.isNewer("0.1.0", "0.1.0"))
        assertTrue(UpdateLogic.isNewer("v0.2.0", "0.1.0"))
    }

    @Test
    fun preReleasesAreNeverOffered() {
        assertFalse(UpdateLogic.isNewer("1.0.1-rc.1", "1.0.0"))
        assertFalse(UpdateLogic.isNewer("v2.0.0-beta", "1.0.0"))
        assertTrue(UpdateLogic.isNewer("1.0.0", "1.0.0-rc.1"))
    }

    @Test
    fun devBuildsAndInterval() {
        assertTrue(UpdateLogic.isDevBuild("0.1.0-dev"))
        assertFalse(UpdateLogic.isDevBuild("0.2.0"))
        val step = UpdateLogic.CHECK_INTERVAL_MS
        assertEquals(3L * 60 * 60 * 1000, step)
        assertTrue(UpdateLogic.shouldCheck(1_000, 0))
        assertFalse(UpdateLogic.shouldCheck(10 * step, 10 * step - 1))
        assertTrue(UpdateLogic.shouldCheck(10 * step, 9 * step))
        assertTrue(UpdateLogic.shouldCheck(5 * step, 9 * step)) // clock went backwards
    }

    @Test
    fun readsLatestRelease() {
        val json = """
            {"url":"x","tag_name": "v0.2.0","name":"Hoppy 0.2.0","assets":[
              {"name":"Hoppy-Setup-x64.exe","size":1},
              {"name":"Hoppy-android.apk","size":2},
              {"name":"SHA256SUMS.txt","size":3}]}
        """.trimIndent()
        assertEquals("0.2.0", UpdateLogic.latestVersion(json))
        assertEquals(UpdateLogic.Release("0.2.0", "v0.2.0"), UpdateLogic.latestRelease(json))
        assertEquals(UpdateLogic.Release("0.2.0", "0.2.0"), UpdateLogic.latestRelease(json.replace("v0.2.0", "0.2.0")))
        assertNull(UpdateLogic.latestVersion(json.replace("Hoppy-android.apk", "other.apk")))
        assertNull(UpdateLogic.latestVersion(json.replace("SHA256SUMS.txt", "sums.txt")))
        assertNull(UpdateLogic.latestVersion(json.replace("v0.2.0", "nightly")))
        // The tag goes into a URL: only plain vX.Y.Z is accepted.
        for (bad in listOf("v0.2", "v0.2.0-rc.1", "v0.2.0+b", "v0.2.0/../x", "0.2.0 ", "vv0.2.0")) {
            assertNull(UpdateLogic.latestRelease(json.replace("v0.2.0", bad)), bad)
        }
        assertNull(UpdateLogic.latestVersion("{}"))
    }

    @Test
    fun buildsUrlsFromTheSameTag() {
        assertEquals(
            "https://github.com/Marketto/hoppy/releases/download/v0.2.0/Hoppy-android.apk",
            UpdateLogic.assetUrl("v0.2.0", UpdateLogic.APK_ASSET),
        )
        assertEquals(
            "https://github.com/Marketto/hoppy/releases/download/V1.2.3/SHA256SUMS.txt",
            UpdateLogic.assetUrl("V1.2.3", UpdateLogic.SUMS_ASSET),
        )
        assertNull(UpdateLogic.assetUrl("latest", UpdateLogic.APK_ASSET))
        assertNull(UpdateLogic.assetUrl("v1.2.3/../../x", UpdateLogic.APK_ASSET))
        assertNull(UpdateLogic.assetUrl("", UpdateLogic.APK_ASSET))
    }

    @Test
    fun readsSha256Sums() {
        val a = "a".repeat(64)
        val b = "B".repeat(64)
        val sums = "$a  Hoppy-Setup-x64.exe\n$b *Hoppy-android.apk\r\n${"c".repeat(63)}  SHA256SUMS.txt\n"
        assertEquals("b".repeat(64), UpdateLogic.expectedSha256(sums))
        assertEquals(a, UpdateLogic.expectedSha256(sums, "Hoppy-Setup-x64.exe"))
        assertNull(UpdateLogic.expectedSha256(sums, "SHA256SUMS.txt"))
        assertNull(UpdateLogic.expectedSha256("", "Hoppy-android.apk"))
        assertNull(UpdateLogic.expectedSha256("$a  Hoppy-android.apk.old"))
    }

    @Test
    fun hexAndPercent() {
        assertEquals("00ff10", UpdateLogic.hex(byteArrayOf(0, -1, 16)))
        assertEquals(50, UpdateLogic.percent(5, 10))
        assertEquals(100, UpdateLogic.percent(20, 10))
        assertEquals(-1, UpdateLogic.percent(5, 0))
    }
}
