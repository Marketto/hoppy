package app.hoppy.legal

import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class LegalLogicTest {
    @Test
    fun asksOnFirstRunAndOnHigherVersions() {
        val v1 = LegalVersion(1, 1)
        assertTrue(LegalLogic.needsAcceptance(v1, null))
        assertFalse(LegalLogic.needsAcceptance(v1, v1))
        assertTrue(LegalLogic.needsAcceptance(LegalVersion(2, 1), v1))
        assertTrue(LegalLogic.needsAcceptance(LegalVersion(1, 2), v1))
        assertFalse(LegalLogic.needsAcceptance(v1, LegalVersion(2, 2))) // downgrade: not again
    }

    @Test
    fun picksItalianOnlyForItalian() {
        assertEquals("it", LegalLogic.language("it"))
        assertEquals("it", LegalLogic.language("it-CH"))
        assertEquals("it", LegalLogic.language("IT_it"))
        assertEquals("en", LegalLogic.language("en-US"))
        assertEquals("en", LegalLogic.language("de"))
        assertEquals("en", LegalLogic.language(null))
    }

    @Test
    fun opensOnlyBundledPages() {
        assertTrue(LegalLogic.isBundledPage("file:///android_asset/legal/terms.it.html"))
        assertTrue(LegalLogic.isBundledPage("file:///android_asset/legal/privacy.en.html#gdpr"))
        assertFalse(LegalLogic.isBundledPage("file:///android_asset/legal/../fonts/x.html"))
        assertFalse(LegalLogic.isBundledPage("file:///android_asset/legal/legal.json"))
        assertFalse(LegalLogic.isBundledPage("file:///data/data/app.hoppy/terms.it.html"))
        assertFalse(LegalLogic.isBundledPage("https://example.org/terms.it.html"))
        assertFalse(LegalLogic.isBundledPage(null))
    }

    @Test
    fun handsOnlyHttpsToTheBrowser() {
        assertTrue(LegalLogic.isExternal("https://github.com/Marketto/hoppy"))
        assertFalse(LegalLogic.isExternal("http://example.org"))
        assertFalse(LegalLogic.isExternal("intent://x#Intent;end"))
        assertFalse(LegalLogic.isExternal("javascript:alert(1)"))
        assertFalse(LegalLogic.isExternal("https://a b"))
    }

    @Test
    fun mapsPagesByLanguage() {
        val pages = mapOf(
            "terms" to mapOf("it" to "terms.it.html", "en" to "terms.en.html"),
            "licenses" to mapOf("en" to "licenses.en.html"),
            "privacy" to mapOf("it" to "../x.html"),
        )
        assertEquals("terms.it.html", LegalLogic.pageFile(pages, "terms", "it"))
        assertEquals("licenses.en.html", LegalLogic.pageFile(pages, "licenses", "it"))
        assertNull(LegalLogic.pageFile(pages, "privacy", "it"))
        assertNull(LegalLogic.pageFile(pages, "other", "en"))
    }
}
