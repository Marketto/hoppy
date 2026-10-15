package app.hoppy.security

import android.content.Context
import android.util.Log
import app.hoppy.Prefs
import app.hoppy.R
import app.hoppy.core.DeviceKey
import app.hoppy.core.HandshakeGuard
import app.hoppy.core.Local
import app.hoppy.core.SessionInfo
import app.hoppy.core.Trust
import app.hoppy.core.UnsupportedKeyFileException
import java.io.File
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import kotlin.concurrent.thread

/**
 * Protocol v1 identity of this phone (spec §7.5, ADR 0010): the static X25519 key
 * (`noBackupFilesDir/identity.key`) and the trust store (`noBackupFilesDir/trust.json`),
 * app-private and never in a backup or a device-to-device transfer (a copied key would give
 * two phones one identity; see also `res/xml/data_extraction_rules.xml`), plus one
 * aborted-handshake guard for the whole process. The device id is bound to the key: when a
 * new key is created, a new id goes with it, so peers see a New device, not a changed key.
 * [loadKey] runs at app start (before discovery uses the id).
 */
object Security {
    private const val TAG = "HoppySecurity"
    private const val KEY_FILE = "identity.key"
    private const val TRUST_FILE = "trust.json"
    @Volatile private var cachedKey: DeviceKey? = null
    @Volatile private var cachedTrust: Trust? = null
    /** A key problem the user must hear about (string resource), taken once by the home screen. */
    @Volatile private var keyProblem: Int? = null
    private val guard = HandshakeGuard()

    /** This device for sessions: current id/name from [Prefs] + the persistent key and trust store. */
    fun local(ctx: Context): Local {
        val app = ctx.applicationContext
        val key = key(app) // first: a new key re-generates the id read below
        return Local(Prefs.device(app), key, trust(app), guard = guard)
    }

    /** Files of versions before 2026-10 lived in filesDir (backed up / transferred): move them once. */
    private fun migrate(ctx: Context, name: String): File {
        val target = File(ctx.noBackupFilesDir, name)
        val old = File(ctx.filesDir, name)
        if (old.exists()) {
            if (!target.exists() && !old.renameTo(target)) Log.w(TAG, "cannot move $old")
            if (target.exists()) old.delete()
        }
        return target
    }

    /**
     * The device key. A key file written by a newer app (downgrade) is never overwritten: a
     * throw-away key is used for this run and the user is told. Other read/write errors are
     * retried once, then thrown (callers show the error; nothing is cached).
     */
    @Synchronized
    fun key(ctx: Context): DeviceKey {
        cachedKey?.let { return it }
        val app = ctx.applicationContext
        val file = migrate(app, KEY_FILE)
        val before = runCatching { file.takeIf { it.exists() }?.readText() }.getOrNull()
        val loaded = try {
            loadWithRetry(file)
        } catch (e: UnsupportedKeyFileException) {
            Log.w(TAG, "key file of a newer Hoppy: using a temporary key", e)
            keyProblem = R.string.key_unsupported
            DeviceKey.generate()
        }
        // New key (first run, damaged file replaced, temporary key): new id too.
        if (before == null || before != loaded.toJson()) Prefs.resetDeviceId(app)
        cachedKey = loaded
        return loaded
    }

    private fun loadWithRetry(file: File): DeviceKey = try {
        DeviceKey.loadOrCreate(file)
    } catch (e: UnsupportedKeyFileException) {
        throw e
    } catch (e: IOException) {
        Log.w(TAG, "cannot load $file, retrying", e)
        Thread.sleep(200)
        DeviceKey.loadOrCreate(file)
    }

    /** Loads the key now (app start, before the id is used); errors are reported later. */
    fun loadKey(ctx: Context) {
        try {
            key(ctx)
        } catch (e: Exception) {
            Log.e(TAG, "device key unavailable", e)
            keyProblem = R.string.key_unreadable
        }
    }

    /** Problem with the device key to show once (string resource id), or null. */
    fun takeKeyProblem(): Int? = keyProblem.also { keyProblem = null }

    @Synchronized
    fun trust(ctx: Context): Trust =
        cachedTrust ?: Trust.open(migrate(ctx.applicationContext, TRUST_FILE)).also { cachedTrust = it }

    /** Loads the trust store in the background (app start). */
    fun warm(ctx: Context) {
        val app = ctx.applicationContext
        thread(name = "hoppy-security-warm", isDaemon = true) { runCatching { trust(app) } }
    }

    /** Ids remembered by this phone → verified? (for the ✓ / 🔒 badges). Reads the store. */
    fun trustedIds(ctx: Context): Map<String, Boolean> =
        trust(ctx).snapshot().peers.mapValues { it.value.verified }
}

/**
 * Incoming offers waiting for Accept/Decline whose sender can be verified from the "Verify"
 * notification action (in-process only: the activity is not exported, the session never
 * leaves memory).
 */
object PendingVerifications {
    class Entry(val session: SessionInfo, val onVerified: () -> Unit)

    private val entries = ConcurrentHashMap<Int, Entry>()

    fun put(id: Int, entry: Entry) { entries[id] = entry }
    fun get(id: Int): Entry? = entries[id]
    fun remove(id: Int) { entries.remove(id) }
}
