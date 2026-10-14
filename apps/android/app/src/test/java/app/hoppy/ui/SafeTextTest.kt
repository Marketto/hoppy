package app.hoppy.ui

import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class SafeTextTest {
    private val pkg = "app.hoppy"

    @Test
    fun labelStripsControlsAndBidiOverrides() {
        assertEquals("Laptopfdp.exe", SafeText.label("Lap\ttop‮fdp.exe"))
        assertEquals("ab", SafeText.label("a​b⁦⁩﻿"))
        assertEquals("?", SafeText.label(" ‮\n "))
        assertEquals("abc", SafeText.label("abcdef", max = 3))
        // Truncation never splits a surrogate pair.
        assertEquals("a🐰", SafeText.label("a🐰b", max = 2))
    }

    @Test
    fun offeredFileNameIsWhatWouldBeSaved() {
        assertEquals("x.txt", SafeText.offeredFileName("../../x.txt"))
        assertEquals("photogpj.exe", SafeText.offeredFileName("photo‮gpj.exe"))
        assertEquals("?", SafeText.offeredFileName(".."))
    }

    @Test
    fun outgoingFileNameIsASinglePathComponent() {
        assertEquals("x.xml", SafeText.outgoingFileName("../../shared_prefs/x.xml"))
        assertEquals("x.xml", SafeText.outgoingFileName("..\\..\\x.xml"))
        assertEquals("evil.txt", SafeText.outgoingFileName("evil\u0000‮.txt"))
        assertEquals("file", SafeText.outgoingFileName(".."))
        assertEquals("file", SafeText.outgoingFileName("/"))
        assertEquals("file", SafeText.outgoingFileName(""))
        assertEquals("file", SafeText.outgoingFileName(null))
        assertEquals("a_b.txt", SafeText.outgoingFileName("a:b.txt"))
    }

    @Test
    fun onlyOtherAppsContentUrisAreReadable() {
        assertTrue(SafeText.isReadableShare("content", "com.android.providers.media.documents", pkg))
        assertTrue(SafeText.isReadableShare("CONTENT", "media", pkg))
        assertFalse(SafeText.isReadableShare("file", "", pkg))
        assertFalse(SafeText.isReadableShare("file", null, pkg))
        assertFalse(SafeText.isReadableShare(null, "media", pkg))
        assertFalse(SafeText.isReadableShare("content", null, pkg))
        assertFalse(SafeText.isReadableShare("content", "", pkg))
        assertFalse(SafeText.isReadableShare("content", "app.hoppy", pkg))
        assertFalse(SafeText.isReadableShare("content", "app.hoppy.androidx-startup", pkg))
        assertFalse(SafeText.isReadableShare("content", "App.Hoppy.FileProvider", pkg))
        assertFalse(SafeText.isReadableShare("content", "0" + "@" + "app.hoppy.fileprovider", pkg)) // userId prefix
        assertFalse(SafeText.isReadableShare("content", "app.hoppy.debug.provider", "app.hoppy.debug"))
        assertFalse(SafeText.isReadableShare("content", "app.hoppyx", pkg))
    }
}
