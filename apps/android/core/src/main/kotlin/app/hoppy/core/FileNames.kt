package app.hoppy.core

import java.io.File
import java.util.Locale

/** Receiver-side name sanitizing (spec §4). Vectors: docs/test-vectors.json. */
object FileNames {
    private val RESERVED = setOf("CON", "PRN", "AUX", "NUL") +
        (1..9).map { "COM$it" } + (1..9).map { "LPT$it" }
    private const val MAX_CHARS = 200
    private const val MAX_EXT_CHARS = 20

    /** Null when nothing usable is left (the offer is invalid). Lengths count code points. */
    fun sanitize(raw: String): String? {
        val last = raw.replace('\\', '/').substringAfterLast('/')
        val cleaned = buildString {
            last.codePoints().forEach { cp ->
                when {
                    cp <= 0x1F || cp == 0x7F -> Unit
                    cp < 0x80 && cp.toChar() in "<>:\"|?*" -> append('_')
                    else -> appendCodePoint(cp)
                }
            }
        }
        var name = cleaned.trim { it == ' ' || it == '.' }
        if (name.isEmpty()) return null
        if (name.substringBefore('.').uppercase(Locale.ROOT) in RESERVED) name = "_$name"
        return truncate(name)
    }

    private fun truncate(name: String): String {
        if (name.codePointCount(0, name.length) <= MAX_CHARS) return name
        val dot = name.lastIndexOf('.')
        if (dot > 0) {
            val ext = name.substring(dot)
            val extChars = ext.codePointCount(0, ext.length)
            if (extChars - 1 <= MAX_EXT_CHARS) return take(name.substring(0, dot), MAX_CHARS - extChars) + ext
        }
        return take(name, MAX_CHARS)
    }

    private fun take(s: String, codePoints: Int): String =
        s.substring(0, s.offsetByCodePoints(0, minOf(codePoints, s.codePointCount(0, s.length))))

    /** `name`, `name (1).ext`, `name (2).ext`, … — first that does not exist in [dir]. */
    fun uniqueFile(dir: File, name: String): File {
        val first = File(dir, name)
        if (!first.exists()) return first
        val dot = name.lastIndexOf('.')
        val (stem, ext) = if (dot > 0) name.substring(0, dot) to name.substring(dot) else name to ""
        var n = 1
        while (true) {
            val f = File(dir, "$stem ($n)$ext")
            if (!f.exists()) return f
            n++
        }
    }
}
