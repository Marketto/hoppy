package app.hoppy.core

import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest
import java.security.SecureRandom

/**
 * Wire format: v0 framing (spec §1–6) and the v1 additions (hello version 1, `crypto`
 * fields, spec §7.1). Spec: docs/protocol.md.
 * Keep in lockstep with crates/hoppy-core/src/protocol.rs and tools/hoppy_ref.py.
 */
object Protocol {
    val MAGIC = byteArrayOf(0x48, 0x4F, 0x50, 0x59) // "HOPY"
    /** Callback hello magic (spec §6). */
    val CALLBACK_MAGIC = byteArrayOf(0x48, 0x4F, 0x50, 0x43) // "HOPC"
    /** Callback token: 128 random bits as 32 lowercase hex characters. */
    const val TOKEN_HEX_LEN = 32
    private val random = SecureRandom()
    const val VERSION = 0
    /** Hello version byte of an encrypted (v1) session (spec §7.2). */
    const val VERSION_V1 = 1
    /** `crypto` field of v1 discovery datagrams and callback hellos (spec §7.1). */
    const val CRYPTO_VERSION = 1
    /**
     * Transition period (ADR 0010): v1 nodes still talk to v0 peers (old apps) without
     * encryption, flagged in the UI. A later release turns this off (spec §7.1).
     */
    const val ALLOW_PLAINTEXT_DEFAULT = true
    const val DISCOVERY_PORT = 47470
    const val TRANSFER_PORT = 47471
    const val MULTICAST_GROUP = "239.255.72.80"
    const val MAX_JSON_FRAME = 1 shl 20
    /** Max JSON frame of the callback hello (spec §6); the real one is under 200 bytes. */
    const val MAX_CALLBACK_HELLO_FRAME = 1024
    const val MAX_FILES = 10_000
    const val HASH_LEN = 32
    const val CHUNK = 256 * 1024

    fun writeHello(out: OutputStream) {
        out.write(MAGIC)
        out.write(VERSION)
    }

    /** v0 only (what an old node accepts). */
    fun readHello(input: InputStream) {
        if (readHelloVersion(input) != VERSION) throw ProtocolException("unsupported version 1")
    }

    /** `HOPY` + version: 0 (plaintext, §3) or 1 (encrypted, §7.2); anything else is an error. */
    fun readHelloVersion(input: InputStream): Int {
        val buf = ByteArray(5)
        DataInputStream(input).readFully(buf)
        if (!buf.copyOfRange(0, 4).contentEquals(MAGIC)) throw ProtocolException("bad magic")
        val v = buf[4].toInt()
        if (v != VERSION && v != VERSION_V1) throw ProtocolException("unsupported version $v")
        return v
    }

    /**
     * `crypto` (spec §7.1): a non-negative integer, capped at 255; any other value counts as
     * absent (null) and never makes a message invalid.
     */
    fun lenientCrypto(v: Any?): Int? = (v as? Long)?.takeIf { it >= 0 }?.coerceAtMost(255)?.toInt()

    fun isValidToken(token: String?): Boolean =
        token != null && token.length == TOKEN_HEX_LEN && token.all { it in '0'..'9' || it in 'a'..'f' }

    fun newToken(): String {
        val bytes = ByteArray(TOKEN_HEX_LEN / 2).also { random.nextBytes(it) }
        return bytes.joinToString("") { "%02x".format(it) }
    }

    /** Constant-time comparison (no early exit on the first differing byte). */
    fun tokensEqual(a: String, b: String): Boolean =
        MessageDigest.isEqual(a.toByteArray(Charsets.UTF_8), b.toByteArray(Charsets.UTF_8))

    /**
     * "HOPC" 0x00 + JSON frame {"id":receiverId,"token":token} (spec §6); v1 receivers
     * append `"crypto":1` (spec §7.1).
     */
    fun writeCallbackHello(out: OutputStream, receiverId: String, token: String, crypto: Boolean = true) {
        out.write(CALLBACK_MAGIC)
        out.write(VERSION)
        val body = linkedMapOf<String, Any?>("id" to receiverId, "token" to token)
        if (crypto) body["crypto"] = CRYPTO_VERSION
        writeJson(out, body)
    }

    /**
     * Does not check the token. The frame is capped at [MAX_CALLBACK_HELLO_FRAME]; the caller
     * bounds the time (spec §6 step 3).
     */
    fun readCallbackHello(input: InputStream): CallbackHello {
        val buf = ByteArray(5)
        DataInputStream(input).readFully(buf)
        if (!buf.copyOfRange(0, 4).contentEquals(CALLBACK_MAGIC)) throw ProtocolException("bad callback magic")
        if (buf[4].toInt() != VERSION) throw ProtocolException("unsupported version ${buf[4]}")
        val m = readJson(input, MAX_CALLBACK_HELLO_FRAME)
        return CallbackHello(m["id"] as? String ?: "", m["token"] as? String ?: "", lenientCrypto(m["crypto"]))
    }

    fun writeJson(out: OutputStream, value: Map<String, Any?>) {
        val body = Json.encode(value).toByteArray(Charsets.UTF_8)
        if (body.size > MAX_JSON_FRAME) throw ProtocolException("frame too large")
        val data = DataOutputStream(out)
        data.writeInt(body.size)
        data.write(body)
        data.flush()
    }

    fun readJson(input: InputStream, maxLen: Int = MAX_JSON_FRAME): Map<String, Any?> {
        val data = DataInputStream(input)
        val len = data.readInt()
        if (len < 0 || len > minOf(maxLen, MAX_JSON_FRAME)) throw ProtocolException("frame of $len bytes exceeds limit")
        val body = ByteArray(len)
        data.readFully(body)
        return try {
            Json.decodeObject(String(body, Charsets.UTF_8))
        } catch (e: JsonException) {
            throw ProtocolException("bad JSON: ${e.message}")
        }
    }
}

class ProtocolException(message: String) : java.io.IOException(message)

/** The JSON of a callback hello (spec §6); [crypto] is null for v0 receivers. */
data class CallbackHello(val id: String, val token: String, val crypto: Int? = null)

data class DeviceInfo(val id: String, val name: String, val platform: String) {
    fun toMap(): Map<String, Any?> = linkedMapOf("id" to id, "name" to name, "platform" to platform)

    companion object {
        fun from(m: Map<String, Any?>) = DeviceInfo(m.str("id"), m.str("name"), m.str("platform"))
    }
}

data class FileEntry(val name: String, val size: Long)

data class Offer(val sender: DeviceInfo, val files: List<FileEntry>) {
    val totalSize: Long get() = files.sumOf { it.size }

    fun toMap(): Map<String, Any?> = linkedMapOf(
        "sender" to sender.toMap(),
        "files" to files.map { linkedMapOf("name" to it.name, "size" to it.size) },
    )

    companion object {
        @Suppress("UNCHECKED_CAST")
        fun from(m: Map<String, Any?>): Offer {
            val sender = m["sender"] as? Map<String, Any?> ?: throw ProtocolException("missing sender")
            val files = m["files"] as? List<*> ?: throw ProtocolException("missing files")
            return Offer(DeviceInfo.from(sender), files.map {
                val f = it as? Map<String, Any?> ?: throw ProtocolException("bad file entry")
                val size = f.long("size")
                if (size < 0) throw ProtocolException("negative size")
                FileEntry(f.str("name"), size)
            })
        }
    }
}

/**
 * Discovery datagram (spec §2). [token] and [to] are set only on [CALLBACK] (spec §6),
 * where [port] is the sender's callback listener. [crypto]: v1 nodes send 1 (spec §7.1);
 * null when absent or not a non-negative integer.
 */
data class DiscoveryMessage(
    val type: String,
    val device: DeviceInfo,
    val port: Int,
    val token: String? = null,
    val to: String? = null,
    val crypto: Int? = null,
) {
    fun encode(): ByteArray = Json.encode(
        linkedMapOf<String, Any?>(
            "hoppy" to Protocol.VERSION, "type" to type,
            "id" to device.id, "name" to device.name, "platform" to device.platform, "port" to port,
        ).apply {
            if (token != null) put("token", token)
            if (to != null) put("to", to)
            if (crypto != null) put("crypto", crypto)
        },
    ).toByteArray(Charsets.UTF_8)

    companion object {
        const val ANNOUNCE = "announce"
        const val QUERY = "query"
        const val BYE = "bye"
        const val CALLBACK = "callback"

        /** Null for anything that is not a valid v0 message (spec §2 rule 5). */
        fun decode(bytes: ByteArray, length: Int = bytes.size): DiscoveryMessage? = try {
            val m = Json.decodeObject(String(bytes, 0, length, Charsets.UTF_8))
            val type = m.str("type")
            val port = m.long("port")
            val token = m["token"] as? String
            val to = m["to"] as? String
            val crypto = Protocol.lenientCrypto(m["crypto"])
            when {
                m.long("hoppy") != Protocol.VERSION.toLong() -> null
                type != ANNOUNCE && type != QUERY && type != BYE && type != CALLBACK -> null
                port !in 0..65535 || m.str("id").isEmpty() -> null
                type == CALLBACK && (port == 0L || !Protocol.isValidToken(token) || to.isNullOrEmpty()) -> null
                type == CALLBACK -> DiscoveryMessage(type, DeviceInfo.from(m), port.toInt(), token, to, crypto)
                else -> DiscoveryMessage(type, DeviceInfo.from(m), port.toInt(), crypto = crypto)
            }
        } catch (e: RuntimeException) {
            null
        } catch (e: ProtocolException) {
            null
        }
    }
}
