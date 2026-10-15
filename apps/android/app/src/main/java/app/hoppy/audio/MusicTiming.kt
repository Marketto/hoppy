package app.hoppy.audio

import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.roundToLong
import kotlin.math.sin

/**
 * Pure timing for [Music] (no `android.*`, unit-tested): bar lengths of the theme loops,
 * bar-aligned switch delays and fade curves.
 */
object MusicTiming {
    const val IDLE = "idle"
    const val TRANSFER = "transfer"
    const val DONE = "done"

    /** A switch closer than this to the next bar boundary waits one more bar (handler jitter). */
    const val MIN_LEAD_MS = 40L

    /**
     * Bar length (ms) per theme (lower-case `HoppyTheme` name) and loop kind.
     * Copied from `assets/music/manifest.json`, which `tools/gen_music.py` writes:
     * regenerate the music → update this table (`MusicTimingTest` fails if they drift).
     */
    val BAR_MS: Map<String, Map<String, Double>> = mapOf(
        "purikura" to mapOf(IDLE to 2222.222, TRANSFER to 1777.778),
        "showa" to mapOf(IDLE to 2448.98, TRANSFER to 1967.213),
        "manga" to mapOf(IDLE to 2142.857, TRANSFER to 1714.286),
        "yume" to mapOf(IDLE to 3000.0, TRANSFER to 2400.0),
        "night" to mapOf(IDLE to 3076.923, TRANSFER to 2448.98),
    )

    fun barMs(theme: String, kind: String): Double? = BAR_MS[theme]?.get(kind)

    /**
     * Milliseconds from [positionMs] (play head of a loop made of whole bars of [barMs])
     * to the next bar boundary; a boundary closer than [minLeadMs] is skipped for the one
     * after it. Unknown bar length (≤ 0) → switch now.
     */
    fun delayToNextBar(positionMs: Long, barMs: Double, minLeadMs: Long = MIN_LEAD_MS): Long {
        if (barMs <= 0.0 || positionMs < 0) return 0
        val intoBar = positionMs.toDouble() % barMs
        var delay = (barMs - intoBar) % barMs
        if (delay < minLeadMs) delay += barMs
        return delay.roundToLong()
    }

    /**
     * Gain at progress [t] (0..1) of a fade from [from] to [to], equal-power shaped so a
     * crossfade of a fade-in and a fade-out keeps the loudness steady.
     */
    fun fadeGain(from: Float, to: Float, t: Float): Float {
        val x = t.coerceIn(0f, 1f)
        return if (to >= from) {
            from + (to - from) * sin(x * PI / 2).toFloat()
        } else {
            to + (from - to) * cos(x * PI / 2).toFloat()
        }
    }
}
