package app.hoppy.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import androidx.appcompat.content.res.AppCompatResources
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.graphics.drawable.IconCompat
import app.hoppy.Prefs
import app.hoppy.R

/**
 * Sharing shortcuts ("Direct Share"): put Hoppy in the top row of the system share sheet
 * instead of at the end of the app grid. res/xml/shortcuts.xml maps the category to
 * [SendActivity]; the share sheet then starts it with `Intent.EXTRA_SHORTCUT_ID`.
 *
 * - [publishNearby]: one generic "Hoppy · nearby" shortcut, pushed at app start.
 * - [onDelivered]: one long-lived shortcut per recent destination device (max 4).
 *
 * The same shortcuts appear on a long-press of the launcher icon; there their intent
 * opens [MainActivity], which shows the file picker (and pre-selects the device).
 * All calls do binder IPC and draw a bitmap: call them off the main thread.
 */
object ShareShortcuts {
    /** Generic shortcut ranks after the devices (ranks are relative, normalised by the OS). */
    private const val NEARBY_RANK = 100

    fun publishNearby(ctx: Context) {
        val app = ctx.applicationContext
        val shortcut = ShortcutInfoCompat.Builder(app, ShareTargets.NEARBY_ID)
            .setShortLabel(app.getString(R.string.shortcut_nearby))
            .setLongLabel(app.getString(R.string.shortcut_nearby_long))
            .setIcon(icon(app))
            .setIntent(MainActivity.sendFilesIntent(app, null))
            .setCategories(setOf(ShareTargets.CATEGORY))
            .setLongLived(true)
            .setRank(NEARBY_RANK)
            .build()
        runCatching { ShortcutManagerCompat.pushDynamicShortcut(app, shortcut) }
    }

    /** After a successful send: move [peerId] to the front of the share-sheet row. */
    @Synchronized
    fun onDelivered(ctx: Context, peerId: String, name: String) {
        if (!ShareTargets.isValidPeerId(peerId)) return
        val app = ctx.applicationContext
        // Leave room for the generic shortcut within the per-activity limit.
        val max = minOf(ShareTargets.MAX_DEVICES, ShortcutManagerCompat.getMaxShortcutCountPerActivity(app) - 1)
        val bumped = ShareTargets.bump(Prefs.recentTargets(app), RecentTarget(peerId, name), max)
        Prefs.setRecentTargets(app, bumped.recent)
        val target = bumped.recent.first()
        val id = ShareTargets.deviceShortcutId(target.peerId)
        val shortcut = ShortcutInfoCompat.Builder(app, id)
            .setShortLabel(target.name)
            .setLongLabel(app.getString(R.string.shortcut_send_to, target.name))
            .setIcon(icon(app))
            .setIntent(MainActivity.sendFilesIntent(app, target.peerId))
            .setCategories(setOf(ShareTargets.CATEGORY))
            .setLongLived(true)
            .setRank(0) // most recent first; ties go to the most recently updated
            .build()
        runCatching {
            if (bumped.evicted.isNotEmpty()) {
                ShortcutManagerCompat.removeLongLivedShortcuts(
                    app,
                    bumped.evicted.map { ShareTargets.deviceShortcutId(it.peerId) },
                )
            }
            ShortcutManagerCompat.pushDynamicShortcut(app, shortcut)
            ShortcutManagerCompat.reportShortcutUsed(app, id)
        }
    }

    /** The share sheet launched us through [shortcutId]: helps the OS ranking. */
    fun reportUsed(ctx: Context, shortcutId: String) {
        runCatching { ShortcutManagerCompat.reportShortcutUsed(ctx.applicationContext, shortcutId) }
    }

    /** Name stored for a recent device, to show while waiting for it to appear. */
    fun recentName(ctx: Context, peerId: String): String? =
        Prefs.recentTargets(ctx).firstOrNull { it.peerId == peerId }?.name

    /**
     * Adaptive bitmap identical to the launcher icon: its generated background and
     * foreground layers (108dp each, tools/gen_mascots.py) stacked; the OS applies its mask.
     */
    private fun icon(ctx: Context): IconCompat {
        val px = (108 * ctx.resources.displayMetrics.density).toInt().coerceIn(108, 432)
        val bitmap = Bitmap.createBitmap(px, px, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        for (layer in intArrayOf(R.drawable.ic_launcher_background, R.drawable.ic_launcher_foreground)) {
            AppCompatResources.getDrawable(ctx, layer)?.run {
                setBounds(0, 0, px, px)
                draw(canvas)
            }
        }
        return IconCompat.createWithAdaptiveBitmap(bitmap)
    }
}
