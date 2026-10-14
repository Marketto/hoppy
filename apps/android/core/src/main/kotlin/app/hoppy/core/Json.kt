package app.hoppy.core

/**
 * Minimal JSON for protocol frames, so this module stays pure JVM (no org.json,
 * which is a stub in Android unit tests). Supports the full JSON grammar;
 * numbers decode to Long when integral, otherwise Double.
 */
object Json {
    fun encode(value: Any?): String = StringBuilder().also { write(it, value) }.toString()

    fun decode(text: String): Any? {
        val p = Parser(text)
        p.skipWs()
        val v = p.value()
        p.skipWs()
        if (!p.atEnd()) throw JsonException("trailing data at ${p.pos}")
        return v
    }

    @Suppress("UNCHECKED_CAST")
    fun decodeObject(text: String): Map<String, Any?> =
        decode(text) as? Map<String, Any?> ?: throw JsonException("expected an object")

    private fun write(sb: StringBuilder, v: Any?) {
        when (v) {
            null -> sb.append("null")
            is String -> writeString(sb, v)
            is Boolean -> sb.append(v)
            is Int, is Long, is Short, is Byte -> sb.append(v.toString())
            is Double, is Float -> {
                val d = (v as Number).toDouble()
                require(d.isFinite()) { "non-finite number" }
                sb.append(if (d == Math.floor(d) && Math.abs(d) < 1e15) d.toLong().toString() else d.toString())
            }
            is Map<*, *> -> {
                sb.append('{')
                var first = true
                for ((k, value) in v) {
                    if (!first) sb.append(',')
                    first = false
                    writeString(sb, k as String)
                    sb.append(':')
                    write(sb, value)
                }
                sb.append('}')
            }
            is Iterable<*> -> {
                sb.append('[')
                var first = true
                for (item in v) {
                    if (!first) sb.append(',')
                    first = false
                    write(sb, item)
                }
                sb.append(']')
            }
            else -> throw IllegalArgumentException("cannot encode ${v::class}")
        }
    }

    private fun writeString(sb: StringBuilder, s: String) {
        sb.append('"')
        for (c in s) {
            when (c) {
                '"' -> sb.append("\\\"")
                '\\' -> sb.append("\\\\")
                '\n' -> sb.append("\\n")
                '\r' -> sb.append("\\r")
                '\t' -> sb.append("\\t")
                else -> if (c < ' ' || c == '\u007f') sb.append(String.format("\\u%04x", c.code)) else sb.append(c)
            }
        }
        sb.append('"')
    }

    private class Parser(val s: String) {
        var pos = 0

        fun atEnd() = pos >= s.length

        fun skipWs() {
            while (pos < s.length && s[pos] in " \t\r\n") pos++
        }

        fun value(): Any? {
            skipWs()
            if (atEnd()) throw JsonException("unexpected end")
            return when (val c = s[pos]) {
                '{' -> obj()
                '[' -> arr()
                '"' -> str()
                't' -> literal("true", true)
                'f' -> literal("false", false)
                'n' -> literal("null", null)
                else -> if (c == '-' || c.isDigit()) num() else throw JsonException("unexpected '$c' at $pos")
            }
        }

        private fun literal(word: String, v: Any?): Any? {
            if (!s.startsWith(word, pos)) throw JsonException("bad literal at $pos")
            pos += word.length
            return v
        }

        private fun obj(): Map<String, Any?> {
            pos++ // {
            val map = LinkedHashMap<String, Any?>()
            skipWs()
            if (peek() == '}') { pos++; return map }
            while (true) {
                skipWs()
                if (peek() != '"') throw JsonException("expected key at $pos")
                val key = str()
                skipWs()
                expect(':')
                map[key] = value()
                skipWs()
                when (next()) {
                    ',' -> continue
                    '}' -> return map
                    else -> throw JsonException("expected , or } at ${pos - 1}")
                }
            }
        }

        private fun arr(): List<Any?> {
            pos++ // [
            val list = ArrayList<Any?>()
            skipWs()
            if (peek() == ']') { pos++; return list }
            while (true) {
                list.add(value())
                skipWs()
                when (next()) {
                    ',' -> continue
                    ']' -> return list
                    else -> throw JsonException("expected , or ] at ${pos - 1}")
                }
            }
        }

        private fun str(): String {
            pos++ // opening quote
            val sb = StringBuilder()
            while (true) {
                if (atEnd()) throw JsonException("unterminated string")
                val c = s[pos++]
                when {
                    c == '"' -> return sb.toString()
                    c == '\\' -> {
                        if (atEnd()) throw JsonException("bad escape")
                        when (val e = s[pos++]) {
                            '"' -> sb.append('"')
                            '\\' -> sb.append('\\')
                            '/' -> sb.append('/')
                            'b' -> sb.append('\b')
                            'f' -> sb.append('\u000c')
                            'n' -> sb.append('\n')
                            'r' -> sb.append('\r')
                            't' -> sb.append('\t')
                            'u' -> {
                                if (pos + 4 > s.length) throw JsonException("bad \\u escape")
                                sb.append(s.substring(pos, pos + 4).toInt(16).toChar())
                                pos += 4
                            }
                            else -> throw JsonException("bad escape \\$e")
                        }
                    }
                    c < ' ' -> throw JsonException("control character in string")
                    else -> sb.append(c)
                }
            }
        }

        private fun num(): Any {
            val start = pos
            if (peek() == '-') pos++
            while (pos < s.length && (s[pos].isDigit() || s[pos] in ".eE+-")) pos++
            val text = s.substring(start, pos)
            return text.toLongOrNull() ?: text.toDoubleOrNull() ?: throw JsonException("bad number $text")
        }

        private fun peek(): Char = if (atEnd()) throw JsonException("unexpected end") else s[pos]
        private fun next(): Char = peek().also { pos++ }
        private fun expect(c: Char) {
            if (next() != c) throw JsonException("expected '$c' at ${pos - 1}")
        }
    }
}

class JsonException(message: String) : RuntimeException(message)

/** Typed accessors for decoded JSON objects. */
internal fun Map<String, Any?>.str(key: String): String =
    this[key] as? String ?: throw ProtocolException("missing string '$key'")

internal fun Map<String, Any?>.long(key: String): Long =
    (this[key] as? Number)?.toLong() ?: throw ProtocolException("missing number '$key'")

internal fun Map<String, Any?>.bool(key: String): Boolean =
    this[key] as? Boolean ?: throw ProtocolException("missing boolean '$key'")
