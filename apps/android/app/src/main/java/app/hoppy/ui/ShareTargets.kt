package app.hoppy.ui

/** A device the user recently sent to; shown as a share-sheet shortcut. */
data class RecentTarget(val peerId: String, val name: String)

/** Result of [ShareTargets.bump]: the new most-recent-first list and what fell off it. */
data class Bumped(val recent: List<RecentTarget>, val evicted: List<RecentTarget>)

/**
 * Pure logic behind the share-sheet ("Direct Share") shortcuts: shortcut ids, the
 * most-recent-first device list and its storage format. No `android.*` here so it is
 * unit-tested on the JVM; the Android side lives in `ShareShortcuts`.
 */
object ShareTargets {
    /** Must match the `<share-target>` category in res/xml/shortcuts.xml. */
    const val CATEGORY = "app.hoppy.category.SHARE_TARGET"

    /** The generic "Hoppy · nearby" shortcut published at app start. */
    const val NEARBY_ID = "hoppy:nearby"

    /** Devices kept as shortcuts (the share sheet shows only a few per app anyway). */
    const val MAX_DEVICES = 4

    private const val DEVICE_PREFIX = "device:"
    private const val MAX_ID = 64
    private const val MAX_NAME = 64

    fun deviceShortcutId(peerId: String): String = DEVICE_PREFIX + peerId

    /** The peer id behind a device shortcut id, or null for other/invalid ids. */
    fun peerIdOf(shortcutId: String?): String? {
        if (shortcutId == null || !shortcutId.startsWith(DEVICE_PREFIX)) return null
        return shortcutId.removePrefix(DEVICE_PREFIX).takeIf(::isValidPeerId)
    }

    /**
     * Peer ids come from the network (protocol says UUID v4, but never trust it):
     * only short ASCII ids become shortcut ids or get stored.
     */
    fun isValidPeerId(id: String): Boolean =
        id.length in 1..MAX_ID && id.all { (it in 'a'..'z') || (it in 'A'..'Z') || (it in '0'..'9') || it in "-_." }

    /** Label-safe device name: no control characters, trimmed, bounded, never empty. */
    fun cleanName(name: String): String =
        name.filterNot { it.isISOControl() }.trim().take(MAX_NAME).ifEmpty { "?" }

    /** Moves (or adds) [target] to the front and keeps at most [max] entries. */
    fun bump(recent: List<RecentTarget>, target: RecentTarget, max: Int = MAX_DEVICES): Bumped {
        val clean = target.copy(name = cleanName(target.name))
        val all = listOf(clean) + recent.filterNot { it.peerId == clean.peerId }
        val keep = all.take(max.coerceAtLeast(1))
        return Bumped(keep, all.drop(keep.size))
    }

    /** One `id<TAB>name` per line; names are cleaned so they cannot contain either. */
    fun encode(recent: List<RecentTarget>): String =
        recent.joinToString("\n") { "${it.peerId}\t${cleanName(it.name)}" }

    /** Inverse of [encode]; skips malformed lines and duplicate ids. */
    fun decode(stored: String?): List<RecentTarget> {
        if (stored.isNullOrEmpty()) return emptyList()
        return stored.split('\n').mapNotNull { line ->
            val tab = line.indexOf('\t')
            if (tab <= 0) return@mapNotNull null
            val id = line.substring(0, tab)
            if (!isValidPeerId(id)) null else RecentTarget(id, cleanName(line.substring(tab + 1)))
        }.distinctBy { it.peerId }
    }
}
