package app.hoppy.core

import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest
import java.security.SecureRandom

/**
 * Protocol v0 wire format. Spec: docs/protocol.md.
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
    const val DISCOVERY_PORT = 47470
    const val TRANSFER_PORT = 47471
    const val MULTICAST_GROUP = "239.255.72.80"
    const val MAX_JSON_FRAME = 1 shl 20
    const val MAX_FILES = 10_000
    const val HASH_LEN = 32
    const val CHUNK = 256 * 1024

    fun writeHello(out: OutputStream) {
        out.write(MAGIC)
        out.write(VERSION)
    }

    fun readHello(input: InputStream) {
        val buf = ByteArray(5)
        DataInputStream(input).readFully(buf)
        if (!buf.copyOfRange(0, 4).contentEquals(MAGIC)) throw ProtocolException("bad magic")
        if (buf[4].toInt() != VERSION) throw ProtocolException("unsupported version ${buf[4]}")
    }

    fun isValidToken(token: String?): Boolean =
        token != null && token.length == TOKEN_HEX_LEN && token.all { it in '0'..'9' || it in 'a'..'f' }

    fun newToken(): String {
        val bytes = ByteArray(TOKEN_HEX_LEN / 2).also { random.nextBytes(it) }
        return bytes.joinToString("") { "%02x".format(it) }
    }

    /** Constant-time comparison (no early exit on the first differing byte). */
    fun tokensEqual(a: String, b: String): Boolean =
        MessageDigest.isEqual(a.toByteArray(Charsets.UTF_8), b.toByteArray(Charsets.UTF_8))

    /** "HOPC" 0x00 + JSON frame {"id":receiverId,"token":token}. Spec §6. */
    fun writeCallbackHello(out: OutputStream, receiverId: String, token: String) {
        out.write(CALLBACK_MAGIC)
        out.write(VERSION)
        writeJson(out, linkedMapOf("id" to receiverId, "token" to token))
    }

    /** Returns (receiverId, token); does not check the token. */
    fun readCallbackHello(input: InputStream): Pair<String, String> {
        val buf = ByteArray(5)
        DataInputStream(input).readFully(buf)
        if (!buf.copyOfRange(0, 4).contentEquals(CALLBACK_MAGIC)) throw ProtocolException("bad callback magic")
        if (buf[4].toInt() != VERSION) throw ProtocolException("unsupported version ${buf[4]}")
        val m = Protocol.readJson(input)
        return (m["id"] as? String ?: "") to (m["token"] as? String ?: "")
    }

    fun writeJson(out: OutputStream, value: Map<String, Any?>) {
        val body = Json.encode(value).toByteArray(Charsets.UTF_8)
        if (body.size > MAX_JSON_FRAME) throw ProtocolException("frame too large")
        val data = DataOutputStream(out)
        data.writeInt(body.size)
        data.write(body)
        data.flush()
    }

    fun readJson(input: InputStream): Map<String, Any?> {
        val data = DataInputStream(input)
        val len = data.readInt()
        if (len < 0 || len > MAX_JSON_FRAME) throw ProtocolException("frame of $len bytes exceeds limit")
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
 * where [port] is the sender's callback listener.
 */
data class DiscoveryMessage(
    val type: String,
    val device: DeviceInfo,
    val port: Int,
    val token: String? = null,
    val to: String? = null,
) {
    fun encode(): ByteArray = Json.encode(
        linkedMapOf<String, Any?>(
            "hoppy" to Protocol.VERSION, "type" to type,
            "id" to device.id, "name" to device.name, "platform" to device.platform, "port" to port,
        ).apply {
            if (token != null) put("token", token)
            if (to != null) put("to", to)
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
            when {
                m.long("hoppy") != Protocol.VERSION.toLong() -> null
                type != ANNOUNCE && type != QUERY && type != BYE && type != CALLBACK -> null
                port !in 0..65535 || m.str("id").isEmpty() -> null
                type == CALLBACK && (port == 0L || !Protocol.isValidToken(token) || to.isNullOrEmpty()) -> null
                type == CALLBACK -> DiscoveryMessage(type, DeviceInfo.from(m), port.toInt(), token, to)
                else -> DiscoveryMessage(type, DeviceInfo.from(m), port.toInt())
            }
        } catch (e: RuntimeException) {
            null
        } catch (e: ProtocolException) {
            null
        }
    }
}
