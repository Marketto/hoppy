package app.hoppy

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.net.wifi.WifiManager
import android.os.Build
import android.provider.Settings
import app.hoppy.audio.Music
import app.hoppy.core.DeviceInfo
import app.hoppy.core.DiscoveryNode
import app.hoppy.core.Peer
import app.hoppy.ui.RecentTarget
import app.hoppy.ui.ShareTargets
import app.hoppy.ui.theme.HoppyTheme
import app.hoppy.update.Updater
import java.util.UUID
import java.util.concurrent.CopyOnWriteArraySet

class HoppyApp : Application() {
    override fun onCreate() {
        super.onCreate()
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannels(
            listOf(
                NotificationChannel(CH_RECEIVER, getString(R.string.channel_receiver), NotificationManager.IMPORTANCE_LOW),
                NotificationChannel(CH_INCOMING, getString(R.string.channel_incoming), NotificationManager.IMPORTANCE_HIGH),
                // "Received files": default importance = system default sound. The id stays
                // "done" so users keep any per-channel setting they already made.
                NotificationChannel(CH_DONE, getString(R.string.channel_done), NotificationManager.IMPORTANCE_DEFAULT),
                // "Updates": new version available, download progress (silent), errors.
                NotificationChannel(CH_UPDATES, getString(R.string.channel_updates), NotificationManager.IMPORTANCE_DEFAULT),
            ),
        )
        Music.init(this)
        Updater.init(this)
    }

    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        Music.trimMemory()
    }

    companion object {
        const val CH_RECEIVER = "receiver"
        const val CH_INCOMING = "incoming"
        const val CH_DONE = "done"
        const val CH_UPDATES = "updates"
    }
}

/** Small preference store: identity, theme, auto-accept, music, update check. */
object Prefs {
    private fun sp(ctx: Context) = ctx.getSharedPreferences("hoppy", Context.MODE_PRIVATE)

    fun device(ctx: Context): DeviceInfo {
        val p = sp(ctx)
        val id = p.getString("device_id", null) ?: UUID.randomUUID().toString().also {
            p.edit().putString("device_id", it).apply()
        }
        return DeviceInfo(id, deviceName(ctx), "android")
    }

    fun deviceName(ctx: Context): String =
        sp(ctx).getString("device_name", null)
            ?: Settings.Global.getString(ctx.contentResolver, Settings.Global.DEVICE_NAME)
            ?: Build.MODEL

    fun setDeviceName(ctx: Context, name: String) {
        sp(ctx).edit().putString("device_name", name.trim().take(64)).apply()
    }

    /** Stored theme; pre-v2 values KAWAII / PRO migrate to PURIKURA / NIGHT. */
    fun theme(ctx: Context): HoppyTheme =
        HoppyTheme.fromStored(sp(ctx).getString("theme", null)) ?: HoppyTheme.DEFAULT

    fun setTheme(ctx: Context, theme: HoppyTheme) {
        sp(ctx).edit().putString("theme", theme.name).apply()
    }

    /**
     * First-run flag: the theme picker is the "installation-time" choice. Users of the
     * old two-theme app already picked (or saw) a theme, so a stored theme counts.
     */
    fun themeChosen(ctx: Context): Boolean =
        sp(ctx).getBoolean("theme_chosen", false) || sp(ctx).contains("theme")

    fun setThemeChosen(ctx: Context, value: Boolean) {
        sp(ctx).edit().putBoolean("theme_chosen", value).apply()
    }

    /** Off by default: anyone on the network could otherwise push files. */
    fun autoAccept(ctx: Context): Boolean = sp(ctx).getBoolean("auto_accept", false)

    fun setAutoAccept(ctx: Context, value: Boolean) {
        sp(ctx).edit().putBoolean("auto_accept", value).apply()
    }

    /** Theme music on the sender (overflow menu); on by default. */
    fun music(ctx: Context): Boolean = sp(ctx).getBoolean("music", true)

    fun setMusic(ctx: Context, value: Boolean) {
        sp(ctx).edit().putBoolean("music", value).apply()
    }

    /** Devices recently sent to, most recent first (share-sheet shortcuts, `ShareShortcuts`). */
    fun recentTargets(ctx: Context): List<RecentTarget> =
        ShareTargets.decode(sp(ctx).getString("recent_targets", null))

    fun setRecentTargets(ctx: Context, list: List<RecentTarget>) {
        sp(ctx).edit().putString("recent_targets", ShareTargets.encode(list)).apply()
    }

    /** Last update check (epoch ms), see `update/Updater.kt`. */
    fun updateCheckedAt(ctx: Context): Long = sp(ctx).getLong("update_checked_at", 0L)

    fun setUpdateCheckedAt(ctx: Context, value: Long) {
        sp(ctx).edit().putLong("update_checked_at", value).apply()
    }

    /** Newest release version seen on GitHub (without the leading "v"). */
    fun latestVersion(ctx: Context): String? = sp(ctx).getString("update_latest", null)

    /** Release tag of [latestVersion] as published (e.g. "v0.2.0"): the APK is fetched from it. */
    fun latestTag(ctx: Context): String? = sp(ctx).getString("update_latest_tag", null)

    fun setLatestRelease(ctx: Context, version: String, tag: String) {
        sp(ctx).edit().putString("update_latest", version).putString("update_latest_tag", tag).apply()
    }

    /** Version the "update available" notification was posted for (once per version). */
    fun updateNotified(ctx: Context): String? = sp(ctx).getString("update_notified", null)

    fun setUpdateNotified(ctx: Context, value: String) {
        sp(ctx).edit().putString("update_notified", value).apply()
    }

    /** Secret carried by our own update notification (MainActivity is exported). */
    fun updateToken(ctx: Context): String {
        val p = sp(ctx)
        return p.getString("update_token", null) ?: UUID.randomUUID().toString().also {
            p.edit().putString("update_token", it).apply()
        }
    }
}

/**
 * Process-wide discovery: one UDP socket on port 47470 shared by screens and the
 * receiver service (reference counted), plus the Wi-Fi multicast lock.
 * Screens subscribe here, not on the DiscoveryNode, so a restart keeps listeners.
 */
object Node {
    private var discovery: DiscoveryNode? = null
    private var multicastLock: WifiManager.MulticastLock? = null
    private var users = 0
    private val listeners = CopyOnWriteArraySet<(List<Peer>) -> Unit>()
    private val forward: (List<Peer>) -> Unit = { peers -> listeners.forEach { it(peers) } }

    /** TCP port of the running receiver, 0 when not receiving (send-only). */
    @Volatile var receivePort: Int = 0
        set(value) {
            field = value
            discovery?.transferPort = value
        }

    fun addListener(l: (List<Peer>) -> Unit) { listeners += l }
    fun removeListener(l: (List<Peer>) -> Unit) { listeners -= l }
    fun peers(): List<Peer> = discovery?.peers().orEmpty()
    fun refresh() { discovery?.refresh() }

    @Synchronized
    fun acquire(ctx: Context) {
        users++
        if (discovery != null) return
        val app = ctx.applicationContext
        multicastLock = app.getSystemService(WifiManager::class.java)
            ?.createMulticastLock("hoppy-discovery")
            ?.apply { setReferenceCounted(false); acquire() }
        discovery = newNode(app)
    }

    @Synchronized
    fun release() {
        if (users == 0) return
        users--
        if (users == 0) {
            discovery?.stop()
            discovery = null
            multicastLock?.release()
            multicastLock = null
        }
    }

    /** Restart with the current identity (after a rename). */
    @Synchronized
    fun restart(ctx: Context) {
        val old = discovery ?: return
        old.removeListener(forward)
        old.stop()
        discovery = newNode(ctx.applicationContext)
    }

    private fun newNode(app: Context): DiscoveryNode =
        DiscoveryNode(Prefs.device(app)).also {
            it.transferPort = receivePort
            it.addListener(forward)
            it.start()
        }
}
