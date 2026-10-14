package app.hoppy.update

/**
 * Pure helpers for the in-app updater (no android.* so they are unit-tested on the JVM).
 * The Android side lives in `Updater.kt`.
 */
object UpdateLogic {
    /** Stable release asset names (docs/release.md): never rename them. */
    const val APK_ASSET = "Hoppy-android.apk"
    const val SUMS_ASSET = "SHA256SUMS.txt"

    const val CHECK_INTERVAL_MS = 24L * 60 * 60 * 1000

    /** Local/CI builds that were not stamped by the release workflow never check. */
    fun isDevBuild(versionName: String): Boolean = versionName.contains("-dev")

    /** At most once per [CHECK_INTERVAL_MS]; a clock that went backwards also allows a check. */
    fun shouldCheck(nowMs: Long, lastCheckMs: Long): Boolean =
        lastCheckMs <= 0L || nowMs < lastCheckMs || nowMs - lastCheckMs >= CHECK_INTERVAL_MS

    /** `v0.2.0` → `0.2.0`; null when not a plausible semantic version. */
    fun normalize(version: String?): String? {
        val v = version?.trim()?.removePrefix("v")?.removePrefix("V") ?: return null
        return if (parse(v) != null) v else null
    }

    /**
     * Semantic-version comparison (`MAJOR.MINOR.PATCH[-pre][+build]`, leading `v` ignored,
     * missing minor/patch = 0). A pre-release is older than the same version without one;
     * pre-release identifiers compare numerically when both are numbers, else as text.
     * Returns null when either side is not a version.
     */
    fun compare(a: String, b: String): Int? {
        val x = parse(a.trim().removePrefix("v").removePrefix("V")) ?: return null
        val y = parse(b.trim().removePrefix("v").removePrefix("V")) ?: return null
        for (i in 0 until 3) {
            val c = x.core[i].compareTo(y.core[i])
            if (c != 0) return c.sign()
        }
        return comparePre(x.pre, y.pre)
    }

    /**
     * True only when [remote] is a valid release (no pre-release part, never offered, as on
     * Windows) strictly newer than [current].
     */
    fun isNewer(remote: String?, current: String): Boolean {
        if (remote == null) return false
        val r = parse(remote.trim().removePrefix("v").removePrefix("V")) ?: return false
        return r.pre.isEmpty() && (compare(remote, current) ?: 0) > 0
    }

    private class Version(val core: List<Long>, val pre: List<String>)

    private val CORE = Regex("""^(\d{1,9})(?:\.(\d{1,9}))?(?:\.(\d{1,9}))?$""")
    private val IDENT = Regex("""^[0-9A-Za-z-]+$""")

    private fun parse(s: String): Version? {
        val noBuild = s.substringBefore('+')
        val core = noBuild.substringBefore('-')
        val pre = if (noBuild.contains('-')) noBuild.substringAfter('-') else null
        val m = CORE.matchEntire(core) ?: return null
        val parts = (1..3).map { m.groupValues[it].ifEmpty { "0" }.toLong() }
        val ids = pre?.split('.') ?: emptyList()
        if (ids.any { !IDENT.matches(it) }) return null
        return Version(parts, ids)
    }

    private fun comparePre(a: List<String>, b: List<String>): Int {
        if (a.isEmpty() || b.isEmpty()) return when {
            a.isEmpty() && b.isEmpty() -> 0
            a.isEmpty() -> 1 // release > pre-release
            else -> -1
        }
        for (i in 0 until minOf(a.size, b.size)) {
            val na = a[i].toLongOrNull()
            val nb = b[i].toLongOrNull()
            val c = when {
                na != null && nb != null -> na.compareTo(nb)
                na != null -> -1 // numeric identifiers sort before alphanumeric ones
                nb != null -> 1
                else -> a[i].compareTo(b[i])
            }
            if (c != 0) return c.sign()
        }
        return a.size.compareTo(b.size).sign()
    }

    private fun Int.sign() = when {
        this > 0 -> 1
        this < 0 -> -1
        else -> 0
    }

    /** A release seen on GitHub: [version] without the `v`, [tag] exactly as published. */
    data class Release(val version: String, val tag: String)

    /** Only plain `vX.Y.Z` / `X.Y.Z` tags are accepted: the tag ends up in a download URL. */
    private val TAG = Regex("""^[vV]?\d{1,9}\.\d{1,9}\.\d{1,9}$""")

    fun isValidTag(tag: String?): Boolean = tag != null && TAG.matches(tag)

    private const val DOWNLOAD_BASE = "https://github.com/Marketto/hoppy/releases/download/"

    /**
     * Download URL of [asset] in the release [tag] (not `releases/latest/download`, so the
     * checksums and the APK always come from the same release); null for an invalid tag.
     */
    fun assetUrl(tag: String, asset: String): String? =
        if (isValidTag(tag)) "$DOWNLOAD_BASE$tag/$asset" else null

    /**
     * Newest release from the `releases/latest` JSON, only if it carries both the APK and
     * SHA256SUMS.txt. Regex instead of org.json so this stays testable off-device; the
     * response is untrusted, so the tag must match [TAG].
     */
    fun latestRelease(json: String): Release? {
        val tag = Regex(""""tag_name"\s*:\s*"([^"\\]{1,64})"""").find(json)?.groupValues?.get(1) ?: return null
        if (!isValidTag(tag)) return null
        val hasAll = listOf(APK_ASSET, SUMS_ASSET).all {
            Regex(""""name"\s*:\s*"${Regex.escape(it)}"""").containsMatchIn(json)
        }
        if (!hasAll) return null
        return normalize(tag)?.let { Release(it, tag) }
    }

    /** Version part of [latestRelease]. */
    fun latestVersion(json: String): String? = latestRelease(json)?.version

    /**
     * Expected SHA-256 (lower-case hex) of [asset] from a `sha256sum` listing
     * (`<hex>  <name>` or `<hex> *<name>`), or null if absent/malformed.
     */
    fun expectedSha256(sums: String, asset: String = APK_ASSET): String? =
        sums.lineSequence()
            .map { it.trim() }
            .mapNotNull { Regex("""^([0-9a-fA-F]{64})\s+\*?(.+)$""").matchEntire(it) }
            .firstOrNull { it.groupValues[2].trim() == asset }
            ?.groupValues?.get(1)?.lowercase()

    fun hex(bytes: ByteArray): String = bytes.joinToString("") { "%02x".format(it.toInt() and 0xff) }

    /** Download progress 0..100, or -1 when the size is unknown. */
    fun percent(done: Long, total: Long): Int =
        if (total <= 0) -1 else ((done.coerceIn(0, total) * 100) / total).toInt()
}
