package app.hoppy.core

import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream

/**
 * Protocol v1 session layer (spec §7.2–7.4): `Noise_XX_25519_ChaChaPoly_SHA256` (Noise rev. 34)
 * with the hoppy prologue and the SAS commit/reveal in the payloads, `u16` framing and the
 * encrypted byte stream that carries the unchanged v0 exchange.
 * Keep in lockstep with crates/hoppy-core/src/session.rs and tools/hoppy_ref.py.
 */
object Noise {
    const val PROTOCOL_NAME = "Noise_XX_25519_ChaChaPoly_SHA256"
    const val MAX_MESSAGE = 65535
    const val MAX_PLAINTEXT = MAX_MESSAGE - Crypto.TAG_LEN
    /** Receivers reject longer handshake messages before reading them. */
    const val MAX_HANDSHAKE_MESSAGE = 2048
    const val MAX_HANDSHAKE_PAYLOAD = 1024
    const val MAX_NAME_CHARS = 64
    const val MAX_PLATFORM_CHARS = 16
    private const val DH = 32
    const val MIN_MSG1 = DH
    const val MIN_MSG2 = DH + DH + Crypto.TAG_LEN + Crypto.TAG_LEN
    const val MIN_MSG3 = DH + Crypto.TAG_LEN + Crypto.TAG_LEN

    /** The 5 hello bytes `HOPY 0x01`: a changed hello breaks the handshake. */
    val PROLOGUE: ByteArray = Protocol.MAGIC + Protocol.VERSION_V1.toByte()

    /** `u16 length` + bytes, one write. */
    fun writeFrame(out: OutputStream, msg: ByteArray) {
        require(msg.size <= MAX_MESSAGE) { "Noise message too long" }
        val buf = ByteArray(2 + msg.size)
        buf[0] = (msg.size ushr 8).toByte()
        buf[1] = msg.size.toByte()
        msg.copyInto(buf, 2)
        out.write(buf)
    }

    /** One framed message of at most [max] bytes; null on a clean EOF before the length. */
    fun readFrame(input: InputStream, max: Int): ByteArray? {
        val b0 = input.read()
        if (b0 < 0) return null
        val b1 = input.read()
        if (b1 < 0) throw EOFException("connection closed inside a frame length")
        val len = (b0 shl 8) or b1
        if (len > max) throw ProtocolException("Noise message of $len bytes exceeds $max")
        val buf = ByteArray(len)
        var off = 0
        while (off < len) {
            val n = input.read(buf, off, len - off)
            if (n < 0) throw EOFException("connection closed inside a Noise message")
            off += n
        }
        return buf
    }
}

/** A Noise cipher state: key + implicit 64-bit counter. */
class CipherState internal constructor(private val key: ByteArray?) {
    private var n = 0L

    fun encrypt(ad: ByteArray, plaintext: ByteArray): ByteArray {
        val k = key ?: return plaintext
        return Crypto.seal(k, n, ad, plaintext).also { n++ }
    }

    fun decrypt(ad: ByteArray, ciphertext: ByteArray): ByteArray {
        val k = key ?: return ciphertext
        return Crypto.open(k, n, ad, ciphertext).also { n++ }
    }

    internal val hasKey: Boolean get() = key != null
}

/** One side of `Noise_XX_25519_ChaChaPoly_SHA256` (no PSK); payloads are opaque here. */
internal class NoiseXX(
    private val initiator: Boolean,
    private val staticKey: DeviceKey,
    ephemeralPrivate: ByteArray? = null,
) {
    private val e: ByteArray = ephemeralPrivate?.copyOf() ?: Crypto.randomBytes(32)
    private val ePub: ByteArray = Crypto.x25519Public(e)
    /** The protocol name is exactly 32 bytes: used as h without hashing. */
    var h: ByteArray = Noise.PROTOCOL_NAME.toByteArray(Charsets.US_ASCII)
        private set
    private var ck: ByteArray = h
    private var cs = CipherState(null)
    private var re: ByteArray? = null
    var rs: ByteArray? = null
        private set

    init {
        check(h.size == 32)
        mixHash(Noise.PROLOGUE)
    }

    private fun mixHash(data: ByteArray) {
        h = Crypto.sha256(h, data)
    }

    private fun mixKey(ikm: ByteArray) {
        val (newCk, temp) = Crypto.hkdf2(ck, ikm)
        ck = newCk
        cs = CipherState(temp)
    }

    private fun encryptAndHash(pt: ByteArray): ByteArray = cs.encrypt(h, pt).also { mixHash(it) }

    private fun decryptAndHash(ct: ByteArray): ByteArray = cs.decrypt(h, ct).also { mixHash(ct) }

    private fun dh(public: ByteArray): ByteArray = Crypto.x25519(e, public)

    private fun dhStatic(public: ByteArray): ByteArray = Crypto.x25519(staticKey.secret(), public)

    // -> e
    fun writeMsg1(payload: ByteArray): ByteArray {
        mixHash(ePub)
        return ePub + encryptAndHash(payload)
    }

    fun readMsg1(msg: ByteArray): ByteArray {
        if (msg.size < Noise.MIN_MSG1) throw CryptoException("handshake message 1 too short")
        val remoteE = msg.copyOfRange(0, 32)
        if (Crypto.isLowOrder(remoteE)) throw CryptoException("low-order ephemeral key")
        re = remoteE
        mixHash(remoteE)
        return decryptAndHash(msg.copyOfRange(32, msg.size))
    }

    // <- e, ee, s, es
    fun writeMsg2(payload: ByteArray): ByteArray {
        val remoteE = re ?: error("message 1 not read")
        mixHash(ePub)
        mixKey(dh(remoteE))
        val encS = encryptAndHash(staticKey.public)
        mixKey(dhStatic(remoteE))
        return ePub + encS + encryptAndHash(payload)
    }

    fun readMsg2(msg: ByteArray): ByteArray {
        if (msg.size < Noise.MIN_MSG2) throw CryptoException("handshake message 2 too short")
        val remoteE = msg.copyOfRange(0, 32)
        if (Crypto.isLowOrder(remoteE)) throw CryptoException("low-order ephemeral key")
        re = remoteE
        mixHash(remoteE)
        mixKey(dh(remoteE))
        val remoteS = decryptAndHash(msg.copyOfRange(32, 80))
        if (Crypto.isLowOrder(remoteS)) throw CryptoException("low-order static key")
        rs = remoteS
        mixKey(dh(remoteS))
        return decryptAndHash(msg.copyOfRange(80, msg.size))
    }

    // -> s, se
    fun writeMsg3(payload: ByteArray): ByteArray {
        val remoteE = re ?: error("message 2 not read")
        val encS = encryptAndHash(staticKey.public)
        mixKey(dhStatic(remoteE))
        return encS + encryptAndHash(payload)
    }

    fun readMsg3(msg: ByteArray): ByteArray {
        if (msg.size < Noise.MIN_MSG3) throw CryptoException("handshake message 3 too short")
        val remoteS = decryptAndHash(msg.copyOfRange(0, 48))
        if (Crypto.isLowOrder(remoteS)) throw CryptoException("low-order static key")
        rs = remoteS
        mixKey(dh(remoteS))
        return decryptAndHash(msg.copyOfRange(48, msg.size))
    }

    /** (send, receive) cipher states of this side. */
    fun split(): Pair<CipherState, CipherState> {
        val (k1, k2) = Crypto.hkdf2(ck, ByteArray(0))
        val c1 = CipherState(k1)
        val c2 = CipherState(k2)
        return if (initiator) c1 to c2 else c2 to c1
    }
}

/** What a handshake learned about the other side. */
data class Remote(val device: DeviceInfo, val public: ByteArray, val sas: Sas) {
    val fingerprint: String get() = Crypto.fingerprint(public)

    override fun equals(other: Any?) =
        other is Remote && device == other.device && public.contentEquals(other.public) && sas == other.sas

    override fun hashCode() = device.hashCode() * 31 + public.contentHashCode()
}

private fun identityPayload(me: DeviceInfo, nonce: ByteArray): ByteArray {
    val body = Json.encode(
        linkedMapOf("id" to me.id, "name" to me.name, "platform" to me.platform, "nonce" to Crypto.hex(nonce)),
    ).toByteArray(Charsets.UTF_8)
    if (body.size > Noise.MAX_HANDSHAKE_PAYLOAD) throw IOException("device name too long for the handshake")
    return body
}

private fun decodePayload(payload: ByteArray): Map<String, Any?> = try {
    Json.decodeObject(String(payload, Charsets.UTF_8))
} catch (e: JsonException) {
    throw ProtocolException("handshake payload: ${e.message}")
}

/**
 * Parses a message 2/3 payload: the id must be 1–128 bytes without hidden characters (it is
 * stored and shown); name and platform must be strings and are cleaned and cut.
 */
internal fun parseIdentity(payload: ByteArray): Pair<DeviceInfo, ByteArray> {
    val p = decodePayload(payload)
    val id = p["id"] as? String ?: throw ProtocolException("bad device id in handshake")
    val name = p["name"] as? String ?: throw ProtocolException("bad device name in handshake")
    val platform = p["platform"] as? String ?: throw ProtocolException("bad platform in handshake")
    val nonce = (p["nonce"] as? String)?.let { Crypto.unhex32(it) } ?: throw ProtocolException("bad handshake nonce")
    if (id.isEmpty() || id.toByteArray(Charsets.UTF_8).size > Crypto.MAX_ID_BYTES ||
        Text.display(id, Crypto.MAX_ID_BYTES) != id
    ) {
        throw ProtocolException("bad device id in handshake")
    }
    return DeviceInfo(id, Text.display(name, Noise.MAX_NAME_CHARS), Text.display(platform, Noise.MAX_PLATFORM_CHARS)) to nonce
}

/** Initiator (the sender, also on callback connections) of the v1 handshake. */
class Initiator internal constructor(key: DeviceKey, ephemeral: ByteArray?, private val nonce: ByteArray) {
    private val hs = NoiseXX(true, key, ephemeral)

    constructor(key: DeviceKey) : this(key, null, Crypto.randomBytes(32))

    /** `-> e` with `{"commit":hex(SHA-256("hoppy-commit-v1" ‖ nonce))}`. */
    fun writeMsg1(): ByteArray =
        hs.writeMsg1(Json.encode(mapOf("commit" to Crypto.hex(Crypto.commit(nonce)))).toByteArray(Charsets.UTF_8))

    /** `<- e, ee, s, es` + the responder's identity and nonce: fixes the SAS. */
    fun readMsg2(msg: ByteArray): Remote {
        val (device, responderNonce) = parseIdentity(hs.readMsg2(msg))
        val h2 = hs.h.copyOf()
        return Remote(device, hs.rs!!.copyOf(), Sas.of(Crypto.sasValue(h2, nonce, responderNonce)))
    }

    /** `-> s, se` + our identity and the revealed nonce. */
    fun writeMsg3(me: DeviceInfo): ByteArray = hs.writeMsg3(identityPayload(me, nonce))

    val handshakeHash: ByteArray get() = hs.h.copyOf()

    /** (send, receive) transport cipher states. */
    fun finish(): Pair<CipherState, CipherState> = hs.split()

    companion object {
        /** Fixed ephemeral key and nonce: test vectors only. */
        fun forTest(key: DeviceKey, ephemeral: ByteArray, nonce: ByteArray) = Initiator(key, ephemeral, nonce)
    }
}

/** Responder (the receiver) of the v1 handshake. */
class Responder internal constructor(key: DeviceKey, ephemeral: ByteArray?, private val nonce: ByteArray) {
    private val hs = NoiseXX(false, key, ephemeral)
    private var commit: ByteArray? = null
    private var h2: ByteArray = ByteArray(0)

    constructor(key: DeviceKey) : this(key, null, Crypto.randomBytes(32))

    fun readMsg1(msg: ByteArray) {
        val p = decodePayload(hs.readMsg1(msg))
        commit = (p["commit"] as? String)?.let { Crypto.unhex32(it) } ?: throw ProtocolException("bad commitment")
    }

    fun writeMsg2(me: DeviceInfo): ByteArray =
        hs.writeMsg2(identityPayload(me, nonce)).also { h2 = hs.h.copyOf() }

    /** Reads the initiator's identity and checks its nonce against the commitment. */
    fun readMsg3(msg: ByteArray): Remote {
        val (device, initiatorNonce) = parseIdentity(hs.readMsg3(msg))
        val c = commit ?: throw ProtocolException("no commitment")
        if (!Crypto.constantTimeEquals(Crypto.commit(initiatorNonce), c)) throw CryptoException("SAS commitment mismatch")
        return Remote(device, hs.rs!!.copyOf(), Sas.of(Crypto.sasValue(h2, initiatorNonce, nonce)))
    }

    val handshakeHash: ByteArray get() = hs.h.copyOf()

    fun finish(): Pair<CipherState, CipherState> = hs.split()

    companion object {
        fun forTest(key: DeviceKey, ephemeral: ByteArray, nonce: ByteArray) = Responder(key, ephemeral, nonce)
    }
}

/**
 * Writing half of an encrypted stream: buffers up to [Noise.MAX_PLAINTEXT] bytes per transport
 * message; [flush] sends what is buffered (never an empty message).
 */
class NoiseOutputStream(private val out: OutputStream, private val cipher: CipherState) : OutputStream() {
    private val buf = ByteArray(Noise.MAX_PLAINTEXT)
    private var len = 0

    private fun sendBuffered() {
        if (len == 0) return
        val msg = cipher.encrypt(ByteArray(0), buf.copyOf(len))
        len = 0
        Noise.writeFrame(out, msg)
    }

    override fun write(b: Int) {
        buf[len++] = b.toByte()
        if (len == buf.size) sendBuffered()
    }

    override fun write(b: ByteArray, off: Int, n: Int) {
        var o = off
        var left = n
        while (left > 0) {
            val k = minOf(left, buf.size - len)
            System.arraycopy(b, o, buf, len, k)
            len += k
            o += k
            left -= k
            if (len == buf.size) sendBuffered()
        }
    }

    override fun flush() {
        sendBuffered()
        out.flush()
    }

    override fun close() {
        out.close()
    }
}

/**
 * Reading half of an encrypted stream. A message that fails authentication (tampered,
 * replayed, reordered, truncated) or is empty (spec §7.2: senders never send one) is an
 * error: the session must end.
 */
class NoiseInputStream(private val input: InputStream, private val cipher: CipherState) : InputStream() {
    private var buf = ByteArray(0)
    private var pos = 0

    private fun fill(): Boolean {
        if (pos < buf.size) return true
        val msg = Noise.readFrame(input, Noise.MAX_MESSAGE) ?: return false
        if (msg.size <= Crypto.TAG_LEN) throw ProtocolException("empty or short Noise transport message")
        buf = try {
            cipher.decrypt(ByteArray(0), msg)
        } catch (e: CryptoException) {
            throw CryptoException("encrypted data failed authentication (tampered or corrupted)")
        }
        pos = 0
        return true
    }

    override fun read(): Int = if (fill()) buf[pos++].toInt() and 0xff else -1

    override fun read(b: ByteArray, off: Int, len: Int): Int {
        if (len == 0) return 0
        if (!fill()) return -1
        val n = minOf(len, buf.size - pos)
        System.arraycopy(buf, pos, b, off, n)
        pos += n
        return n
    }

    override fun available(): Int = buf.size - pos

    override fun close() {
        input.close()
    }
}

/** Network text made safe to show (same rules as Rust `display_text`, Python `display_text`). */
object Text {
    /**
     * Hidden characters: controls (Cc), format characters (Cf; ZWNJ/ZWJ are kept, they only
     * join glyphs) and line/paragraph separators. Bidi overrides could disguise names.
     */
    fun isHidden(cp: Int): Boolean {
        val t = Character.getType(cp)
        return t == Character.CONTROL.toInt() || t == Character.LINE_SEPARATOR.toInt() ||
            t == Character.PARAGRAPH_SEPARATOR.toInt() ||
            (t == Character.FORMAT.toInt() && cp != 0x200C && cp != 0x200D)
    }

    /** Hidden characters removed, trimmed, cut to [limit] code points (ending in "…" when cut). */
    fun display(raw: String, limit: Int): String {
        val clean = buildString { raw.codePoints().forEach { if (!isHidden(it)) appendCodePoint(it) } }.trim()
        val count = clean.codePointCount(0, clean.length)
        if (count <= limit) return clean
        val end = clean.offsetByCodePoints(0, maxOf(limit - 1, 0))
        return clean.substring(0, end).trimEnd() + "…"
    }
}
