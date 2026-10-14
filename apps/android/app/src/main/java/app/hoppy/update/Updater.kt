package app.hoppy.update

import android.app.Activity
import android.app.Application
import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.os.Build
import android.os.Bundle
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import app.hoppy.HoppyApp
import app.hoppy.Prefs
import app.hoppy.R
import app.hoppy.ui.MainActivity
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import java.util.concurrent.CopyOnWriteArraySet
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

/**
 * Update check + in-app updater for the APK published on GitHub Releases
 * (docs/platforms/android.md "Updates"). Pure parts are in [UpdateLogic].
 *
 * Flow: [maybeCheck] (home screen start, ≤ once per 24 h, silent on errors) →
 * notification + home banner → user taps → [download] (SHA-256 against SHA256SUMS.txt) →
 * PackageInstaller session → [InstallReceiver] → system confirmation. The system verifies
 * that the new APK is signed with the same key as the installed app.
 */
object Updater {
    private const val REPO = "Marketto/hoppy"
    private const val API_LATEST = "https://api.github.com/repos/$REPO/releases/latest"
    private const val MAX_JSON = 512 * 1024
    private const val MAX_SUMS = 64 * 1024
    private const val MAX_APK = 200L * 1024 * 1024

    const val NOTIF_AVAILABLE = 50
    const val NOTIF_PROGRESS = 51

    private val busy = AtomicBoolean(false)
    private val checking = AtomicBoolean(false)
    private val listeners = CopyOnWriteArraySet<() -> Unit>()
    @Volatile private var visibleActivities = 0

    /** True while a download/install is being prepared. */
    val downloading: Boolean get() = busy.get()

    fun addListener(l: () -> Unit) { listeners += l }
    fun removeListener(l: () -> Unit) { listeners -= l }
    private fun changed() = listeners.forEach { it() }

    /** Tracks whether one of our screens is visible (the install confirmation needs one). */
    fun init(app: Application) {
        app.registerActivityLifecycleCallbacks(object : Application.ActivityLifecycleCallbacks {
            override fun onActivityStarted(activity: Activity) { visibleActivities++ }
            override fun onActivityStopped(activity: Activity) { visibleActivities-- }
            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit
            override fun onActivityResumed(activity: Activity) = Unit
            override fun onActivityPaused(activity: Activity) = Unit
            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit
            override fun onActivityDestroyed(activity: Activity) = Unit
        })
    }

    fun currentVersion(ctx: Context): String =
        runCatching { ctx.packageManager.getPackageInfo(ctx.packageName, 0).versionName }.getOrNull() ?: "0.0.0-dev"

    /** Newer version already known from a previous check (home banner), or null. */
    fun availableVersion(ctx: Context): String? {
        val current = currentVersion(ctx)
        if (UpdateLogic.isDevBuild(current)) return null
        return Prefs.latestVersion(ctx)?.takeIf { UpdateLogic.isNewer(it, current) }
    }

    /**
     * Checks GitHub at most once per 24 h on a background thread; errors are ignored and
     * do not count as a check (the timestamp is stored only after a complete response).
     */
    fun maybeCheck(ctx: Context) {
        val app = ctx.applicationContext
        val current = currentVersion(app)
        if (UpdateLogic.isDevBuild(current)) return
        val now = System.currentTimeMillis()
        if (!UpdateLogic.shouldCheck(now, Prefs.updateCheckedAt(app))) return
        if (!checking.compareAndSet(false, true)) return
        Prefs.updateToken(app) // created here, not concurrently from the check thread
        thread(name = "hoppy-update-check", isDaemon = true) {
            try {
                check(app, current)
            } catch (@Suppress("UNUSED_PARAMETER") e: Exception) {
                // Silent: the timestamp is not stored, so the next start retries.
            } finally {
                checking.set(false)
            }
        }
    }

    private fun check(app: Context, current: String) {
        val json = String(get(API_LATEST, current, json = true).use { readCapped(it, MAX_JSON) }, Charsets.UTF_8)
        // Only a complete response counts as a check: errors retry on the next start.
        Prefs.setUpdateCheckedAt(app, System.currentTimeMillis())
        val latest = UpdateLogic.latestRelease(json) ?: return
        Prefs.setLatestRelease(app, latest.version, latest.tag)
        if (UpdateLogic.isNewer(latest.version, current)) {
            if (Prefs.updateNotified(app) != latest.version) {
                Prefs.setUpdateNotified(app, latest.version)
                notifyAvailable(app, latest.version)
            }
            changed()
        }
    }

    /** Intent for the "update available" notification tap (checked by [isUpdateIntent]). */
    fun updateIntent(ctx: Context): Intent =
        Intent(ctx, MainActivity::class.java)
            .setAction(ACTION_UPDATE)
            .putExtra(EXTRA_TOKEN, Prefs.updateToken(ctx))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)

    /**
     * MainActivity is exported: only our own notification (it carries a secret token from
     * our private prefs) may start a download without a tap on the banner.
     */
    fun isUpdateIntent(ctx: Context, intent: Intent?): Boolean =
        intent?.action == ACTION_UPDATE && intent.getStringExtra(EXTRA_TOKEN) == Prefs.updateToken(ctx)

    private fun notifyAvailable(app: Context, version: String) {
        val tap = PendingIntent.getActivity(
            app, NOTIF_AVAILABLE, updateIntent(app), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val text = app.getString(R.string.update_available, version)
        notify(
            app, NOTIF_AVAILABLE,
            base(app)
                .setContentTitle(text)
                .setStyle(NotificationCompat.BigTextStyle().bigText(text))
                .setCategory(NotificationCompat.CATEGORY_RECOMMENDATION)
                .setContentIntent(tap)
                .setAutoCancel(true)
                .build(),
        )
    }

    /**
     * Downloads the APK of the release seen by the last check (same tag for the APK and
     * SHA256SUMS.txt), verifies the hash and hands it to the PackageInstaller. Call only
     * after a user tap and with install permission granted.
     */
    fun download(ctx: Context) {
        val app = ctx.applicationContext
        if (!busy.compareAndSet(false, true)) return
        changed()
        cancel(app, NOTIF_AVAILABLE)
        thread(name = "hoppy-update-download") {
            val dir = File(app.cacheDir, "updates")
            val apk = File(dir, UpdateLogic.APK_ASSET)
            try {
                val ua = currentVersion(app)
                val tag = Prefs.latestTag(app)
                val sumsUrl = tag?.let { UpdateLogic.assetUrl(it, UpdateLogic.SUMS_ASSET) }
                val apkUrl = tag?.let { UpdateLogic.assetUrl(it, UpdateLogic.APK_ASSET) }
                if (sumsUrl == null || apkUrl == null || UpdateLogic.normalize(tag) != Prefs.latestVersion(app)) {
                    Prefs.setUpdateCheckedAt(app, 0L) // check again on the next start
                    throw IOException("unknown release, try again later")
                }
                progress(app, -1)
                val sums = String(get(sumsUrl, ua).use { readCapped(it, MAX_SUMS) }, Charsets.UTF_8)
                val expected = UpdateLogic.expectedSha256(sums) ?: throw IOException("no checksum for ${UpdateLogic.APK_ASSET}")
                dir.deleteRecursively()
                if (!dir.mkdirs()) throw IOException("cannot create ${dir.name}")
                val actual = fetchApk(app, apkUrl, ua, apk)
                if (actual != expected) {
                    apk.delete()
                    fail(app, app.getString(R.string.update_bad_checksum))
                    return@thread
                }
                val info = app.packageManager.getPackageArchiveInfo(apk.path, 0)
                if (info?.packageName != app.packageName) throw IOException("not a Hoppy APK")
                install(app, apk)
            } catch (e: Exception) {
                fail(app, e.message ?: e.javaClass.simpleName)
            } finally {
                apk.delete()
                busy.set(false)
                changed()
            }
        }
    }

    /** Streams the APK to [dest] with progress; returns its SHA-256 (hex). */
    private fun fetchApk(app: Context, url: String, ua: String, dest: File): String {
        val conn = open(url, ua, json = false)
        try {
            val total = conn.contentLengthLong
            if (total > MAX_APK) throw IOException("download too large")
            val md = MessageDigest.getInstance("SHA-256")
            var done = 0L
            var shown = -2
            var shownAt = 0L
            conn.inputStream.use { input ->
                dest.outputStream().use { out ->
                    val buf = ByteArray(64 * 1024)
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        done += n
                        if (done > MAX_APK) throw IOException("download too large")
                        md.update(buf, 0, n)
                        out.write(buf, 0, n)
                        val pct = UpdateLogic.percent(done, total)
                        val now = System.currentTimeMillis()
                        if (pct != shown && now - shownAt >= 500) {
                            shown = pct
                            shownAt = now
                            progress(app, pct)
                        }
                    }
                }
            }
            if (total > 0 && done != total) throw IOException("download incomplete")
            return UpdateLogic.hex(md.digest())
        } finally {
            conn.disconnect()
        }
    }

    private fun install(app: Context, apk: File) {
        val installer = app.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(app.packageName)
            setSize(apk.length())
            if (Build.VERSION.SDK_INT >= 31) {
                // A self-update may skip the prompt where Android allows it; otherwise
                // the system still asks (STATUS_PENDING_USER_ACTION).
                setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
            }
        }
        val id = installer.createSession(params)
        try {
            installer.openSession(id).use { session ->
                session.openWrite(UpdateLogic.APK_ASSET, 0, apk.length()).use { out ->
                    apk.inputStream().use { it.copyTo(out) }
                    session.fsync(out)
                }
                val status = Intent(app, InstallReceiver::class.java).setAction(InstallReceiver.ACTION_STATUS)
                // Mutable: the installer adds EXTRA_STATUS / EXTRA_INTENT. Explicit target, so safe.
                val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE
                session.commit(PendingIntent.getBroadcast(app, id, status, flags).intentSender)
            }
        } catch (e: Exception) {
            runCatching { installer.abandonSession(id) }
            throw e
        }
    }

    /**
     * The system needs the user's confirmation. Started directly when a Hoppy screen is
     * visible (background activity starts are blocked); otherwise a notification opens it.
     */
    internal fun confirm(ctx: Context, confirmIntent: Intent) {
        if (visibleActivities > 0) {
            val started = runCatching {
                ctx.startActivity(confirmIntent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }.isSuccess
            if (started) {
                cancel(ctx, NOTIF_PROGRESS)
                return
            }
        }
        val tap = PendingIntent.getActivity(
            ctx, NOTIF_PROGRESS, confirmIntent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        notify(
            ctx, NOTIF_PROGRESS,
            base(ctx)
                .setContentTitle(ctx.getString(R.string.update_ready))
                .setContentIntent(tap)
                .setAutoCancel(true)
                .build(),
        )
    }

    internal fun fail(ctx: Context, reason: String) {
        File(ctx.cacheDir, "updates").deleteRecursively()
        val text = ctx.getString(R.string.update_failed, reason.take(200))
        notify(
            ctx, NOTIF_PROGRESS,
            base(ctx)
                .setContentTitle(text)
                .setStyle(NotificationCompat.BigTextStyle().bigText(text))
                .setCategory(NotificationCompat.CATEGORY_ERROR)
                .setContentIntent(
                    PendingIntent.getActivity(
                        ctx, NOTIF_AVAILABLE + 100, Intent(ctx, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
                    ),
                )
                .setAutoCancel(true)
                .build(),
        )
    }

    internal fun cancel(ctx: Context, id: Int) {
        ctx.getSystemService(NotificationManager::class.java).cancel(id)
    }

    private fun progress(ctx: Context, pct: Int) {
        val b = base(ctx)
            .setContentTitle(ctx.getString(R.string.update_downloading))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setCategory(NotificationCompat.CATEGORY_PROGRESS)
            .setProgress(100, pct.coerceAtLeast(0), pct < 0)
        if (pct >= 0) b.setContentText("$pct%")
        notify(ctx, NOTIF_PROGRESS, b.build())
    }

    private fun base(ctx: Context) = NotificationCompat.Builder(ctx, HoppyApp.CH_UPDATES)
        .setSmallIcon(R.drawable.ic_notification)

    private fun notify(ctx: Context, id: Int, n: Notification) {
        runCatching { NotificationManagerCompat.from(ctx).notify(id, n) } // SecurityException if denied
    }

    private fun open(url: String, version: String, json: Boolean): HttpURLConnection {
        val conn = URL(url).openConnection() as HttpURLConnection
        conn.connectTimeout = 15_000
        conn.readTimeout = 30_000
        conn.instanceFollowRedirects = true // github.com → objects.githubusercontent.com (https → https)
        conn.setRequestProperty("User-Agent", "Hoppy/$version")
        conn.setRequestProperty("Accept", if (json) "application/vnd.github+json" else "application/octet-stream")
        val code = conn.responseCode
        if (code != HttpURLConnection.HTTP_OK) {
            conn.disconnect()
            throw IOException("HTTP $code")
        }
        if (conn.url.protocol != "https") {
            conn.disconnect()
            throw IOException("insecure redirect")
        }
        return conn
    }

    private fun get(url: String, version: String, json: Boolean = false): InputStream {
        val conn = open(url, version, json)
        return object : java.io.FilterInputStream(conn.inputStream) {
            override fun close() {
                runCatching { super.close() }
                conn.disconnect()
            }
        }
    }

    private fun readCapped(input: InputStream, max: Int): ByteArray {
        val out = java.io.ByteArrayOutputStream()
        val buf = ByteArray(8 * 1024)
        while (true) {
            val n = input.read(buf)
            if (n < 0) break
            if (out.size() + n > max) throw IOException("response too large")
            out.write(buf, 0, n)
        }
        return out.toByteArray()
    }

    private const val ACTION_UPDATE = "app.hoppy.action.UPDATE"
    private const val EXTRA_TOKEN = "app.hoppy.UPDATE_TOKEN"
}
