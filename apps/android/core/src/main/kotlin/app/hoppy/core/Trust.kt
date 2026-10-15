package app.hoppy.core

import java.io.File
import java.io.IOException
import java.util.TreeMap

/**
 * Trust on first use (spec §7.5) and device verification (§7.6–7.7). Keep in lockstep with
 * crates/hoppy-core/src/trust.rs.
 */
enum class TrustState(val wire: String) {
    /** First contact: never seen. Show the SAS and offer "Verify now" / "Later". */
    NEW("new"),
    /** Seen before with this key, not verified. Show the SAS, offer "Verify". */
    KNOWN("known"),
    /** The user verified this key: show ✓. */
    VERIFIED("verified"),
    /** Seen before with a DIFFERENT key: block (never send, decline without asking) and warn. */
    KEY_CHANGED("key_changed"),
}

/** The other device of an encrypted session, as authenticated by the handshake. */
data class PeerKey(val id: String, val name: String, val platform: String, val fingerprint: String) {
    companion object {
        fun of(remote: Remote) = PeerKey(remote.device.id, remote.device.name, remote.device.platform, remote.fingerprint)
    }
}

/**
 * Security of one session, for the UIs: the receiver's `decide` gets it with the offer, the
 * sender gets it in [SendListener.onSession] before the offer goes out.
 */
data class SessionInfo(
    /** false: v0 peer (old app), nothing is encrypted: show "non cifrato" / "not encrypted". */
    val encrypted: Boolean,
    val peer: PeerKey? = null,
    val trust: TrustState? = null,
    /** Same value on both screens of an honest session. */
    val sas: Sas? = null,
    /** A New device using the name of a device remembered under another id: warn. */
    val nameMatchesKnown: Boolean = false,
) {
    companion object {
        val PLAINTEXT = SessionInfo(encrypted = false)
    }
}

data class TrustEntry(
    val fingerprint: String,
    /** Last name seen with this key (display only). */
    val name: String,
    /** Unix seconds. */
    val firstSeen: Long,
    val verified: Boolean,
    /** `ip:port` where this device last completed an encrypted transfer to it (spec §7.1). */
    val addr: String? = null,
)

/** The persisted store: `{"version":1,"peers":{"<id>":{fingerprint,name,first_seen,verified,addr?}}}`. */
class TrustStore(val version: Int = 1, peers: Map<String, TrustEntry> = emptyMap()) {
    val peers: TreeMap<String, TrustEntry> = TreeMap(peers)

    fun copy() = TrustStore(version, peers)

    fun toJson(): String = Json.encode(
        linkedMapOf(
            "version" to version,
            "peers" to peers.entries.associateTo(LinkedHashMap()) { (id, e) ->
                id to linkedMapOf<String, Any?>(
                    "fingerprint" to e.fingerprint, "name" to e.name, "first_seen" to e.firstSeen, "verified" to e.verified,
                ).apply { if (e.addr != null) put("addr", e.addr) }
            },
        ),
    )

    fun state(id: String, fingerprint: String): TrustState {
        val e = peers[id] ?: return TrustState.NEW
        return when {
            e.fingerprint != fingerprint -> TrustState.KEY_CHANGED
            e.verified -> TrustState.VERIFIED
            else -> TrustState.KNOWN
        }
    }

    /** TOFU: remembers a first-seen key, refreshes name/addr of a known one; never replaces a key. */
    fun pin(peer: PeerKey, addr: String?, now: Long): Boolean {
        val e = peers[peer.id]
        if (e == null) {
            peers[peer.id] = TrustEntry(peer.fingerprint, peer.name, now, false, addr)
            return true
        }
        if (e.fingerprint != peer.fingerprint) return false
        var next = e
        if (e.name != peer.name) next = next.copy(name = peer.name)
        if (addr != null && e.addr != addr) next = next.copy(addr = addr)
        if (next == e) return false
        peers[peer.id] = next
        return true
    }

    fun encryptedAt(addr: String): Boolean = peers.values.any { it.addr == addr }

    /** Another remembered id already uses this name (case-insensitive, trimmed). */
    fun nameTakenByOther(id: String, name: String): Boolean {
        val n = name.trim().lowercase()
        return n.isNotEmpty() && peers.any { (other, e) -> other != id && e.name.trim().lowercase() == n }
    }

    /** Marks [peer]'s key verified (pinning it on first contact); KeyChanged is an error. */
    fun setVerified(peer: PeerKey, now: Long) {
        if (state(peer.id, peer.fingerprint) == TrustState.KEY_CHANGED) throw KeyChangedException(peer)
        pin(peer, null, now)
        peers[peer.id] = peers[peer.id]!!.copy(verified = true)
    }

    /** Replaces whatever is pinned for the id with [peer]'s current key, unverified. */
    fun retrust(peer: PeerKey, now: Long) {
        peers.remove(peer.id)
        pin(peer, null, now)
    }

    fun forget(id: String): Boolean = peers.remove(id) != null

    companion object {
        /** Throws [IOException] for anything that is not a store (the caller sets it aside). */
        fun fromJson(text: String): TrustStore {
            val m = try {
                Json.decodeObject(text)
            } catch (e: JsonException) {
                throw IOException("trust store: ${e.message}")
            }
            val version = (m["version"] as? Long)?.toInt() ?: if ("version" in m) throw IOException("trust store: bad version") else 1
            val raw = m["peers"] ?: emptyMap<String, Any?>()
            val peers = raw as? Map<*, *> ?: throw IOException("trust store: bad peers")
            val out = HashMap<String, TrustEntry>()
            for ((id, v) in peers) {
                val e = v as? Map<*, *> ?: throw IOException("trust store: bad entry")
                val addr = e["addr"]
                if (addr != null && addr !is String) throw IOException("trust store: bad addr")
                out[id as String] = TrustEntry(
                    fingerprint = e["fingerprint"] as? String ?: throw IOException("trust store: no fingerprint"),
                    name = e["name"] as? String ?: throw IOException("trust store: no name"),
                    firstSeen = (e["first_seen"] as? Long)?.takeIf { it >= 0 } ?: throw IOException("trust store: no first_seen"),
                    verified = e["verified"] as? Boolean ?: throw IOException("trust store: no verified"),
                    addr = addr as String?,
                )
            }
            return TrustStore(version, out)
        }
    }
}

/** The peer's key is not the remembered one: never send, decline without asking (spec §7.5). */
class KeyChangedException(val peer: PeerKey) :
    CryptoException("the security key of ${peer.name} has changed (possible impersonation): forget the device to trust the new key")

/** A QR code did not match the key this device knows (spec §7.7). */
open class VerificationFailedException(message: String) : CryptoException(message)

fun unixNow(): Long = System.currentTimeMillis() / 1000

/**
 * Thread-safe trust store of a node, optionally backed by a JSON file that every change
 * re-reads, merges and replaces atomically under a lock file (`<name>.lock`, stale after
 * 10 s), plus the one-time codes of QR codes on screen. A damaged file is set aside as
 * `<name>.bad` (Rust: `trust.json.bad`) and reported once through [takeWarning].
 */
class Trust private constructor(private var store: TrustStore, private val file: File?) {
    private val qrCodes = ArrayList<Pair<String, Long>>() // code, nanoTime
    private var warning: String? = null
    private var stamp: Pair<Long, Long>? = null // (lastModified, length) of the loaded file

    /** Something the user should hear about (damaged file set aside), once. */
    @Synchronized
    fun takeWarning(): String? = warning.also { warning = null }

    /** Picks up changes another process wrote; skipped when the file did not change. */
    private fun reload() {
        val f = file ?: return
        val now = if (f.exists()) f.lastModified() to f.length() else null
        if (now == stamp) return
        val (loaded, w) = readStore(f)
        if (loaded != null) store = loaded
        if (w != null) warning = w
        stamp = if (f.exists()) f.lastModified() to f.length() else null
    }

    private fun <T> update(f: (TrustStore) -> Pair<T, Boolean>): T = synchronized(this) {
        val lock = file?.let { FileLock.acquire(it) }
        try {
            stamp = null // always re-read under the lock
            reload()
            val (out, changed) = f(store)
            if (changed && file != null) {
                writePrivate(file, store.toJson().toByteArray(Charsets.UTF_8))
                stamp = file.lastModified() to file.length()
            }
            out
        } finally {
            lock?.release()
        }
    }

    @Synchronized
    fun snapshot(): TrustStore {
        reload()
        return store.copy()
    }

    fun state(id: String, fingerprint: String): TrustState = snapshot().state(id, fingerprint)

    fun get(id: String): TrustEntry? = snapshot().peers[id]

    /** TOFU pin after an accepted transfer (+ the address it was reached at, sender side). */
    fun pin(peer: PeerKey, addr: String? = null) {
        update { it.pin(peer, addr, unixNow()) to true }
    }

    fun encryptedAt(addr: String): Boolean = snapshot().encryptedAt(addr)

    fun nameTakenByOther(id: String, name: String): Boolean = snapshot().nameTakenByOther(id, name)

    /** "Coincide" / "They match", or a QR scan that matched. [KeyChangedException] if another key is pinned. */
    fun markVerified(peer: PeerKey) {
        update { it.setVerified(peer, unixNow()); Unit to true }
    }

    fun retrust(peer: PeerKey) {
        update { it.retrust(peer, unixNow()); Unit to true }
    }

    /** "Forget this device": the next contact is New again. */
    fun forget(id: String): Boolean = update { s -> s.forget(id).let { it to it } }

    /** Starts showing a QR code (spec §7.7): returns the payload to render. */
    @Synchronized
    fun beginQr(me: DeviceInfo, key: DeviceKey): String {
        val code = Crypto.newQrCode()
        pruneQr()
        if (qrCodes.size >= MAX_QR_CODES) qrCodes.removeAt(0)
        qrCodes += code to System.nanoTime()
        return QrPayload(me.id, key.fingerprint, code).encode()
    }

    /** The QR code was closed: its codes are no longer accepted. */
    @Synchronized
    fun endQr() = qrCodes.clear()

    private fun pruneQr() {
        val now = System.nanoTime()
        qrCodes.removeAll { now - it.second >= QR_CODE_TTL_NS }
    }

    /** Consumes a live one-time code (constant-time compare). */
    @Synchronized
    fun takeQrCode(code: String): Boolean {
        pruneQr()
        val given = code.toByteArray(Charsets.UTF_8)
        var found = -1
        qrCodes.forEachIndexed { i, (c, _) -> if (Crypto.constantTimeEquals(c.toByteArray(Charsets.UTF_8), given)) found = i }
        if (found >= 0) qrCodes.removeAt(found)
        return found >= 0
    }

    /**
     * Scanner side, local part of spec §7.7: the scanned fingerprint must equal the key this
     * device knows for that id — [live] (the session on screen) when its id matches, else
     * the pinned one. On a match the key is marked verified.
     */
    fun checkQr(payload: String, live: PeerKey?): Pair<QrPayload, PeerKey> {
        val qr = QrPayload.parse(payload) ?: throw IllegalArgumentException("not a Hoppy verification code")
        val known = live?.takeIf { it.id == qr.id } ?: get(qr.id)?.let { PeerKey(qr.id, it.name, "", it.fingerprint) }
            ?: throw UnknownDeviceException(qr)
        if (known.fingerprint != qr.fingerprint) {
            throw VerificationFailedException("the code does not match the key of ${known.name} (possible interception)")
        }
        markVerified(known)
        return qr to known
    }

    companion object {
        const val QR_CODE_TTL_MS = 600_000L
        private const val QR_CODE_TTL_NS = QR_CODE_TTL_MS * 1_000_000
        private const val MAX_QR_CODES = 4

        fun inMemory(store: TrustStore = TrustStore()) = Trust(store, null)

        /** Backed by [file] (created on the first change). A damaged file is set aside, not fatal. */
        fun open(file: File): Trust {
            val (store, warning) = readStore(file)
            return Trust(store ?: TrustStore(), file).also {
                it.warning = warning
                it.stamp = if (file.exists()) file.lastModified() to file.length() else null
            }
        }

        private fun readStore(file: File): Pair<TrustStore?, String?> {
            if (!file.exists()) return null to null
            val text = try {
                file.readText()
            } catch (e: IOException) {
                return null to null
            }
            return try {
                TrustStore.fromJson(text) to null
            } catch (e: IOException) {
                val bad = File(file.parentFile, file.name + ".bad")
                file.renameTo(bad)
                null to "the list of known devices was damaged (${e.message}); it was set aside as ${bad.name}"
            }
        }
    }
}

/** The scanned device was never seen and no session with it is on screen. */
class UnknownDeviceException(val qr: QrPayload) :
    VerificationFailedException("unknown device: exchange a file with it first, then scan again")

/** An exclusive lock file next to the store, so two processes do not undo each other. */
private class FileLock private constructor(private val path: File) {
    fun release() {
        path.delete()
    }

    companion object {
        private const val STALE_MS = 10_000L
        private const val WAIT_MS = 3_000L

        fun acquire(store: File): FileLock {
            val path = File(store.parentFile, store.name + ".lock")
            path.parentFile?.mkdirs()
            val start = System.currentTimeMillis()
            while (true) {
                if (path.createNewFile()) return FileLock(path)
                val age = System.currentTimeMillis() - path.lastModified()
                if (path.exists() && age > STALE_MS) {
                    path.delete()
                    continue
                }
                if (System.currentTimeMillis() - start > WAIT_MS) {
                    throw IOException("the list of known devices is locked by another Hoppy process")
                }
                Thread.sleep(15)
            }
        }
    }
}

/**
 * Receiver-side limits of protocol v1 (spec §7.3 "Aborted handshakes"): at most
 * [MAX_ABORTS_PER_IP] per source IP and [MAX_ABORTS_TOTAL] in total in any 60 s; reaching the
 * per-IP cap blocks that IP for 60 s, doubling per strike (≤ 1 h; strikes forgotten after 1 h).
 * Also rate-limits user warnings. Same rules as Rust `guard.rs`. Times are monotonic ms.
 */
class HandshakeGuard {
    private class PerIp {
        val aborts = ArrayDeque<Long>()
        var strikes = 0
        var lastStrike: Long? = null
        var blockedUntil: Long? = null
    }

    private val ips = HashMap<String, PerIp>()
    private val total = ArrayDeque<Long>()
    private val warned = HashMap<String, Long>()

    private fun prune(now: Long) {
        while (total.isNotEmpty() && now - total.first() >= WINDOW_MS) total.removeFirst()
        val it = ips.entries.iterator()
        while (it.hasNext()) {
            val p = it.next().value
            while (p.aborts.isNotEmpty() && now - p.aborts.first() >= WINDOW_MS) p.aborts.removeFirst()
            if (p.lastStrike?.let { now - it >= STRIKE_MEMORY_MS } == true) {
                p.strikes = 0
                p.lastStrike = null
            }
            if (p.aborts.isEmpty() && p.strikes == 0 && (p.blockedUntil ?: Long.MIN_VALUE) <= now) it.remove()
        }
        if (warned.size > MAX_TRACKED) warned.entries.removeIf { now - it.value >= WARNING_INTERVAL_MS }
    }

    /** null if a v1 handshake from [ip] may start now, else the recent abort count to report. */
    @Synchronized
    fun admit(ip: String, now: Long): Int? {
        prune(now)
        if (total.size >= MAX_ABORTS_TOTAL) return total.size
        val p = ips[ip] ?: return null
        if ((p.blockedUntil ?: Long.MIN_VALUE) > now) return maxOf(p.aborts.size, MAX_ABORTS_PER_IP)
        if (p.aborts.size >= MAX_ABORTS_PER_IP) return p.aborts.size
        return null
    }

    /** A handshake from [ip] ended after message 2 without a first frame; returns recent aborts from it. */
    @Synchronized
    fun recordAbort(ip: String, now: Long): Int {
        prune(now)
        total.addLast(now)
        if (ips.size >= MAX_TRACKED && ip !in ips) return 1 // too many sources: the global cap still applies
        val p = ips.getOrPut(ip) { PerIp() }
        p.aborts.addLast(now)
        if (p.aborts.size >= MAX_ABORTS_PER_IP) {
            p.strikes++
            p.lastStrike = now
            val factor = 1L shl minOf(p.strikes - 1, 6)
            p.blockedUntil = now + minOf(BACKOFF_START_MS * factor, BACKOFF_MAX_MS)
        }
        return p.aborts.size
    }

    /** true if a warning with this key may be shown now (at most once per [WARNING_INTERVAL_MS]). */
    @Synchronized
    fun shouldWarn(key: String, now: Long): Boolean {
        val last = warned[key]
        if (last != null && now - last < WARNING_INTERVAL_MS) return false
        warned[key] = now
        return true
    }

    companion object {
        const val MAX_ABORTS_PER_IP = 5
        const val MAX_ABORTS_TOTAL = 20
        const val WARNING_INTERVAL_MS = 60_000L
        private const val WINDOW_MS = 60_000L
        private const val BACKOFF_START_MS = 60_000L
        private const val BACKOFF_MAX_MS = 3_600_000L
        private const val STRIKE_MEMORY_MS = 3_600_000L
        private const val MAX_TRACKED = 1024

        fun nowMs(): Long = System.nanoTime() / 1_000_000
    }
}
