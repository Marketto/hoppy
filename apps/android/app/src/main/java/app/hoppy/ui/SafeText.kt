package app.hoppy.ui

import app.hoppy.core.FileNames
import java.util.Locale

/**
 * Pure checks for text and URIs that come from outside the app (peers on the LAN, other
 * apps' share intents, content providers). No `android.*` here so it is unit-tested on
 * the JVM (`SafeTextTest`).
 */
object SafeText {
    private const val MAX_LABEL = 64
    private const val FALLBACK_FILE = "file"
    private const val OWN_PREFIX = "app.hoppy"

    /** ISO controls and Unicode format characters (bidi overrides such as U+202E, ZW*). */
    fun isInvisible(cp: Int): Boolean =
        Character.isISOControl(cp) || Character.getType(cp) == Character.FORMAT.toInt()

    /** [s] without [isInvisible] code points. */
    fun stripInvisible(s: String): String = buildString {
        s.codePoints().forEach { if (!isInvisible(it)) appendCodePoint(it) }
    }

    /** Label-safe name (device or sender): visible characters only, trimmed, bounded, never empty. */
    fun label(name: String, max: Int = MAX_LABEL): String {
        val clean = stripInvisible(name).trim()
        val end = clean.offsetByCodePoints(0, minOf(max, clean.codePointCount(0, clean.length)))
        return clean.substring(0, end).trim().ifEmpty { "?" }
    }

    /**
     * Name of a file offered to us (incoming notification): the protocol §4 sanitizer
     * (same as what gets written), then [label] so bidi tricks cannot disguise the extension.
     */
    fun offeredFileName(raw: String): String = label(FileNames.sanitize(raw) ?: "?", max = 200)

    /**
     * Name to use for a file we read from another app's provider (its `DISPLAY_NAME`, or
     * the URI's last segment): only the last path segment, through the §4 sanitizer, never
     * `.`/`..`, never empty. Safe as a single path component.
     */
    fun outgoingFileName(raw: String?): String {
        val name = raw?.let { FileNames.sanitize(stripInvisible(it)) }
        return if (name == null || name == "." || name == "..") FALLBACK_FILE else name
    }

    /**
     * Whether Hoppy may read a URI another app handed it. Only `content:` URIs of other
     * apps' providers: no `file:` (would read our own private files) and no provider of
     * our own package (`app.hoppy*`, also with a `userId` + "@" prefix, which the resolver accepts).
     */
    fun isReadableShare(scheme: String?, authority: String?, ownPackage: String): Boolean {
        if (scheme == null || !scheme.equals("content", ignoreCase = true)) return false
        val host = authority?.substringAfterLast('@')?.trim()?.lowercase(Locale.ROOT)
        if (host.isNullOrEmpty()) return false
        val own = ownPackage.lowercase(Locale.ROOT)
        return !(host.startsWith(OWN_PREFIX) || host == own || host.startsWith("$own."))
    }
}
