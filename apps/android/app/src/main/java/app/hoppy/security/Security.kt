package app.hoppy.security

import android.content.Context
import android.util.Log
import app.hoppy.Prefs
import app.hoppy.core.DeviceKey
import app.hoppy.core.HandshakeGuard
import app.hoppy.core.Local
import app.hoppy.core.PeerKey
import app.hoppy.core.SessionInfo
import app.hoppy.core.Trust
import java.io.File
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArraySet
import kotlin.concurrent.thread

/**
 * Protocol v1 identity of this phone (spec §7.5, ADR 0010): the static X25519 key
 * (`filesDir/identity.key`) and the trust store (`filesDir/trust.json`), both app-private,
 * plus one aborted-handshake guard for the whole process. First use reads the files: call
 * [local] off the main thread ([warm] does it at app start).
 */
object Security {
    private const val TAG = "HoppySecurity"
    @Volatile private var cachedKey: DeviceKey? = null
    @Volatile private var cachedTrust: Trust? = null
    private val guard = HandshakeGuard()
    private val verifiedListeners = CopyOnWriteArraySet<(PeerKey) -> Unit>()

    /** This device for sessions: current id/name from [Prefs] + the persistent key and trust store. */
    fun local(ctx: Context): Local {
        val app = ctx.applicationContext
        return Local(Prefs.device(app), key(app), trust(app), guard = guard)
    }

    @Synchronized
    fun key(ctx: Context): DeviceKey = cachedKey ?: run {
        val file = File(ctx.applicationContext.filesDir, "identity.key")
        val loaded = try {
            DeviceKey.loadOrCreate(file)
        } catch (e: IOException) {
            // A key file of a newer version (the app was downgraded): never overwrite it.
            // Run with a throw-away key; peers will see a changed key until Hoppy is updated.
            Log.w(TAG, "cannot use $file", e)
            DeviceKey.generate()
        }
        loaded.also { cachedKey = it }
    }

    @Synchronized
    fun trust(ctx: Context): Trust =
        cachedTrust ?: Trust.open(File(ctx.applicationContext.filesDir, "trust.json")).also { cachedTrust = it }

    /** Loads the key and the trust store in the background (app start). */
    fun warm(ctx: Context) {
        val app = ctx.applicationContext
        thread(name = "hoppy-security-warm", isDaemon = true) { runCatching { local(app) } }
    }

    /** Ids remembered by this phone → verified? (for the ✓ / 🔒 badges). Reads the store. */
    fun trustedIds(ctx: Context): Map<String, Boolean> =
        trust(ctx).snapshot().peers.mapValues { it.value.verified }

    fun addVerifiedListener(l: (PeerKey) -> Unit) { verifiedListeners += l }
    fun removeVerifiedListener(l: (PeerKey) -> Unit) { verifiedListeners -= l }

    /** A device that scanned our QR code proved it (receiver side): close the QR screen with ✓. */
    fun notifyVerified(peer: PeerKey) = verifiedListeners.forEach { runCatching { it(peer) } }
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
