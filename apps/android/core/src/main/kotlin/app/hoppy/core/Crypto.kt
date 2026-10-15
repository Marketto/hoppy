package app.hoppy.core

import org.bouncycastle.crypto.InvalidCipherTextException
import org.bouncycastle.crypto.modes.ChaCha20Poly1305
import org.bouncycastle.crypto.params.AEADParameters
import org.bouncycastle.crypto.params.KeyParameter
import org.bouncycastle.math.ec.rfc7748.X25519
import java.io.File
import java.io.IOException
import java.nio.file.FileAlreadyExistsException
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.nio.file.attribute.PosixFilePermissions
import java.security.MessageDigest
import java.security.SecureRandom
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * Protocol v1 primitives (spec §7, ADR 0010): X25519 (RFC 7748), ChaCha20-Poly1305
 * (RFC 8439), SHA-256, HMAC-SHA256, plus fingerprints, SAS, QR payloads and the device key.
 * Pure JVM: X25519 and the AEAD come from the BouncyCastle lightweight API (constant time,
 * no JCA provider needed: Android 10 has no X25519 KeyAgreement), hashes from the JDK.
 * Keep in lockstep with crates/hoppy-core/src/crypto.rs and tools/hoppy_ref.py.
 */
object Crypto {
    const val KEY_LEN = 32
    const val TAG_LEN = 16
    /** Unpadded RFC 4648 base32 of a SHA-256 (256 bits → 52 characters). */
    const val FINGERPRINT_LEN = 52
    const val QR_PREFIX = "hoppy:verify:1:"
    /** One-time QR code: 128 random bits as 32 lowercase hex characters. */
    const val QR_CODE_HEX_LEN = 32
    /** Longest device id accepted in a QR payload or a handshake (UTF-8 bytes). */
    const val MAX_ID_BYTES = 128

    private const val BASE32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
    private val SAS_LABEL = "hoppy-sas-v1".toByteArray(Charsets.US_ASCII)
    private val COMMIT_LABEL = "hoppy-commit-v1".toByteArray(Charsets.US_ASCII)
    private val random = SecureRandom()

    fun randomBytes(n: Int): ByteArray = ByteArray(n).also { random.nextBytes(it) }

    // ------------------------------------------------------------------ X25519

    /** Public key of a 32-byte private key (clamped as RFC 7748 says). */
    fun x25519Public(secret: ByteArray): ByteArray {
        require(secret.size == KEY_LEN)
        return ByteArray(KEY_LEN).also { X25519.scalarMultBase(secret, 0, it, 0) }
    }

    /** RFC 7748 X25519(k, u). The caller rejects low-order `u` ([isLowOrder]). */
    fun x25519(secret: ByteArray, public: ByteArray): ByteArray {
        require(secret.size == KEY_LEN && public.size == KEY_LEN)
        return ByteArray(KEY_LEN).also { X25519.scalarMult(secret, 0, public, 0, it, 0) }
    }

    private val LOW_ORDER_PROBE = ByteArray(KEY_LEN) { 0x5a }

    /**
     * Spec §7.3: a small-order public key (any clamped scalar, a multiple of the cofactor 8,
     * maps it to the all-zero point). Such keys make a Diffie-Hellman output predictable.
     */
    fun isLowOrder(public: ByteArray): Boolean {
        if (public.size != KEY_LEN) return true
        val out = x25519(LOW_ORDER_PROBE, public)
        return out.fold(0) { acc, b -> acc or b.toInt() } == 0
    }

    // ------------------------------------------------------------------ AEAD

    /** Noise nonce: 4 zero bytes + the 64-bit counter, little-endian. */
    private fun noiseNonce(n: Long): ByteArray {
        val out = ByteArray(12)
        for (i in 0 until 8) out[4 + i] = (n ushr (8 * i)).toByte()
        return out
    }

    /** ChaCha20-Poly1305 with the Noise nonce for counter [n]: ciphertext ‖ 16-byte tag. */
    fun seal(key: ByteArray, n: Long, ad: ByteArray, plaintext: ByteArray): ByteArray =
        aead(true, key, noiseNonce(n), ad, plaintext)

    /** Opens what [seal] produced; [CryptoException] if it fails authentication. */
    fun open(key: ByteArray, n: Long, ad: ByteArray, sealed: ByteArray): ByteArray {
        if (sealed.size < TAG_LEN) throw CryptoException("ciphertext too short")
        return aead(false, key, noiseNonce(n), ad, sealed)
    }

    /** RFC 8439 §2.8 AEAD with an explicit 12-byte nonce (KATs). */
    fun aead(encrypt: Boolean, key: ByteArray, nonce: ByteArray, ad: ByteArray, input: ByteArray): ByteArray {
        val c = ChaCha20Poly1305()
        c.init(encrypt, AEADParameters(KeyParameter(key), TAG_LEN * 8, nonce, ad))
        val out = ByteArray(c.getOutputSize(input.size))
        return try {
            var len = c.processBytes(input, 0, input.size, out, 0)
            len += c.doFinal(out, len)
            if (len == out.size) out else out.copyOf(len)
        } catch (e: InvalidCipherTextException) {
            throw CryptoException("authentication failed (tampered or corrupted data)")
        }
    }

    // ------------------------------------------------------------------ hashes

    fun sha256(vararg parts: ByteArray): ByteArray {
        val d = MessageDigest.getInstance("SHA-256")
        parts.forEach { d.update(it) }
        return d.digest()
    }

    fun hmacSha256(key: ByteArray, vararg parts: ByteArray): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        // An empty key is legal for HMAC but not for SecretKeySpec: HMAC pads it to zeros anyway.
        mac.init(SecretKeySpec(if (key.isEmpty()) ByteArray(64) else key, "HmacSHA256"))
        parts.forEach { mac.update(it) }
        return mac.doFinal()
    }

    /** RFC 5869 HKDF-SHA256 (KAT only; Noise uses [hkdf2]). */
    fun hkdf(salt: ByteArray, ikm: ByteArray, info: ByteArray, length: Int): ByteArray {
        val prk = hmacSha256(if (salt.isEmpty()) ByteArray(32) else salt, ikm)
        val out = java.io.ByteArrayOutputStream()
        var block = ByteArray(0)
        var i = 1
        while (out.size() < length) {
            block = hmacSha256(prk, block, info, byteArrayOf(i.toByte()))
            out.write(block)
            i++
        }
        return out.toByteArray().copyOf(length)
    }

    /** Noise HKDF with two outputs (spec rev. 34 §4.3). */
    fun hkdf2(ck: ByteArray, ikm: ByteArray): Pair<ByteArray, ByteArray> {
        val temp = hmacSha256(ck, ikm)
        val o1 = hmacSha256(temp, byteArrayOf(1))
        return o1 to hmacSha256(temp, o1, byteArrayOf(2))
    }

    // ------------------------------------------------------------------ encodings

    /** RFC 4648 base32, upper case, no padding. */
    fun base32(data: ByteArray): String {
        val sb = StringBuilder((data.size + 4) / 5 * 8)
        var buffer = 0
        var bits = 0
        for (b in data) {
            buffer = (buffer shl 8) or (b.toInt() and 0xff)
            bits += 8
            while (bits >= 5) {
                bits -= 5
                sb.append(BASE32[(buffer ushr bits) and 31])
            }
            buffer = buffer and ((1 shl bits) - 1)
        }
        if (bits > 0) sb.append(BASE32[(buffer shl (5 - bits)) and 31])
        return sb.toString()
    }

    fun hex(data: ByteArray): String = data.joinToString("") { "%02x".format(it.toInt() and 0xff) }

    /** Lower-case hex only (what every implementation writes); null otherwise. */
    fun unhex(s: String): ByteArray? {
        if (s.length % 2 != 0 || !s.all { it in '0'..'9' || it in 'a'..'f' }) return null
        return ByteArray(s.length / 2) { s.substring(2 * it, 2 * it + 2).toInt(16).toByte() }
    }

    fun unhex32(s: String): ByteArray? = unhex(s)?.takeIf { it.size == 32 }

    /** Spec §7.5: base32(SHA-256(static public key)), 52 characters. */
    fun fingerprint(public: ByteArray): String = base32(sha256(public))

    fun isFingerprint(s: String): Boolean = s.length == FINGERPRINT_LEN && s.all { it in BASE32 }

    /** Commitment to the initiator's SAS nonce (spec §7.3). */
    fun commit(nonce: ByteArray): ByteArray = sha256(COMMIT_LABEL, nonce)

    /** Spec §7.6: first 24 bits of SHA-256("hoppy-sas-v1" ‖ h2 ‖ n_i ‖ n_r). */
    fun sasValue(h2: ByteArray, initiatorNonce: ByteArray, responderNonce: ByteArray): Int {
        val d = sha256(SAS_LABEL, h2, initiatorNonce, responderNonce)
        return ((d[0].toInt() and 0xff) shl 16) or ((d[1].toInt() and 0xff) shl 8) or (d[2].toInt() and 0xff)
    }

    /** Constant-time comparison (no early exit). */
    fun constantTimeEquals(a: ByteArray, b: ByteArray): Boolean = MessageDigest.isEqual(a, b)

    fun newQrCode(): String = hex(randomBytes(QR_CODE_HEX_LEN / 2))
}

/** A handshake or transport message failed a cryptographic check. */
open class CryptoException(message: String) : IOException(message)

/** One entry of the SAS emoji table (spec §7.6). */
data class SasEmoji(val emoji: String, val it: String, val en: String)

/**
 * The short authentication string of a session (spec §7.6): a 24-bit value shown as 4 emoji
 * (6 bits each, most significant first) or as 8 zero-padded digits "1234 5678".
 */
data class Sas(val value: Int) {
    /** Indices into [TABLE]. */
    val emoji: List<Int> = listOf(18, 12, 6, 0).map { (value ushr it) and 63 }
    val text: String = emoji.joinToString("") { TABLE[it].emoji }
    val digits: String = "%08d".format(value).let { "${it.substring(0, 4)} ${it.substring(4)}" }

    /** Emoji names for screen readers / reading aloud. */
    fun words(italian: Boolean): List<String> = emoji.map { if (italian) TABLE[it].it else TABLE[it].en }

    companion object {
        fun of(value: Int) = Sas(value and 0xFFFFFF)

        private fun e(emoji: String, it: String, en: String) = SasEmoji(emoji, it, en)

        /** Single source of truth: docs/protocol.md §7.6 and test-vectors.json `v1.sas_emoji`. */
        val TABLE: List<SasEmoji> = listOf(
            e("🐘", "elefante", "elephant"), e("🐢", "tartaruga", "turtle"), e("🐙", "polpo", "octopus"),
            e("🦋", "farfalla", "butterfly"), e("🐌", "lumaca", "snail"), e("🐝", "ape", "bee"),
            e("🐧", "pinguino", "penguin"), e("🐳", "balena", "whale"), e("🦀", "granchio", "crab"),
            e("🐍", "serpente", "snake"), e("🦉", "gufo", "owl"), e("🐪", "cammello", "camel"),
            e("🐇", "coniglio", "rabbit"), e("🐓", "gallo", "rooster"), e("🐞", "coccinella", "ladybug"),
            e("🐖", "maiale", "pig"), e("🐒", "scimmia", "monkey"), e("🍎", "mela", "apple"),
            e("🍌", "banana", "banana"), e("🍇", "uva", "grapes"), e("🥜", "arachidi", "peanuts"),
            e("🍋", "limone", "lemon"), e("🍍", "ananas", "pineapple"), e("🍿", "popcorn", "popcorn"),
            e("🥕", "carota", "carrot"), e("☕", "caffè", "coffee"), e("🍄", "fungo", "mushroom"),
            e("🥑", "avocado", "avocado"), e("🍕", "pizza", "pizza"), e("🍔", "hamburger", "hamburger"),
            e("🍩", "ciambella", "doughnut"), e("🍦", "gelato", "ice cream"), e("🎂", "torta", "cake"),
            e("🎲", "dado", "dice"), e("🎁", "regalo", "gift"), e("🔑", "chiave", "key"),
            e("🔔", "campana", "bell"), e("🎸", "chitarra", "guitar"), e("🎺", "tromba", "trumpet"),
            e("⚽", "pallone", "ball"), e("🚲", "bicicletta", "bicycle"), e("🚀", "razzo", "rocket"),
            e("⛵", "barca a vela", "sailboat"), e("⌛", "clessidra", "hourglass"), e("💡", "lampadina", "light bulb"),
            e("📷", "fotocamera", "camera"), e("🔨", "martello", "hammer"), e("⚓", "àncora", "anchor"),
            e("👑", "corona", "crown"), e("🏠", "casa", "house"), e("🌵", "cactus", "cactus"),
            e("🌻", "girasole", "sunflower"), e("🍀", "quadrifoglio", "four-leaf clover"), e("🌳", "albero", "tree"),
            e("🌴", "palma", "palm tree"), e("🍁", "foglia d'acero", "maple leaf"), e("⛄", "pupazzo di neve", "snowman"),
            e("⭐", "stella", "star"), e("⚡", "fulmine", "lightning"), e("🌈", "arcobaleno", "rainbow"),
            e("🌊", "onda", "wave"), e("🔥", "fuoco", "fire"), e("🌍", "mappamondo", "globe"),
            e("🌋", "vulcano", "volcano"),
        )
    }
}

/** The QR verification payload (spec §7.7): `hoppy:verify:1:<id>:<fingerprint>:<code>`. */
data class QrPayload(val id: String, val fingerprint: String, val code: String) {
    fun encode(): String = "${Crypto.QR_PREFIX}$id:$fingerprint:$code"

    companion object {
        /** Null for anything that is not a well-formed v1 payload; the id may contain ':'. */
        fun parse(text: String): QrPayload? {
            val t = text.trim()
            if (!t.startsWith(Crypto.QR_PREFIX)) return null
            val rest = t.substring(Crypto.QR_PREFIX.length)
            val c2 = rest.lastIndexOf(':')
            if (c2 < 0) return null
            val c1 = rest.lastIndexOf(':', c2 - 1)
            if (c1 < 0) return null
            val id = rest.substring(0, c1)
            // ASCII-only upper case (like Rust's to_ascii_uppercase): "ı" or "ß" must not turn
            // into valid base32 letters.
            val fp = rest.substring(c1 + 1, c2).map { if (it in 'a'..'z') it - ('a' - 'A') else it }
                .joinToString("")
            val code = rest.substring(c2 + 1)
            val codeOk = code.length == Crypto.QR_CODE_HEX_LEN && Crypto.unhex(code) != null
            val idOk = id.isNotEmpty() && id.toByteArray(Charsets.UTF_8).size <= Crypto.MAX_ID_BYTES &&
                id.none { Character.isISOControl(it) }
            return if (codeOk && idOk && Crypto.isFingerprint(fp)) QrPayload(id, fp, code) else null
        }
    }
}

/** A key file written by a newer app version: never overwritten. */
class UnsupportedKeyFileException(message: String) : IOException(message)

/**
 * The device's long-term X25519 key pair (its v1 identity, spec §7.5). Created once on the
 * first run and kept in app-private storage; never sent anywhere.
 */
class DeviceKey private constructor(private val secret: ByteArray) {
    val public: ByteArray = Crypto.x25519Public(secret)
    val fingerprint: String get() = Crypto.fingerprint(public)

    internal fun secret(): ByteArray = secret

    /** `{"version":1,"x25519_private":"<64 hex>"}`. */
    fun toJson(): String = Json.encode(linkedMapOf("version" to 1, "x25519_private" to Crypto.hex(secret)))

    override fun toString(): String = "DeviceKey($fingerprint)"

    companion object {
        fun generate(): DeviceKey = DeviceKey(Crypto.randomBytes(32))

        fun fromSecret(secret: ByteArray): DeviceKey {
            require(secret.size == 32) { "X25519 private keys have 32 bytes" }
            return DeviceKey(secret.copyOf())
        }

        /** Parses a key file; another `version` is [UnsupportedKeyFileException]. */
        fun fromJson(text: String): DeviceKey {
            val m = try {
                Json.decodeObject(text)
            } catch (e: JsonException) {
                throw IOException("key file: ${e.message}")
            }
            val version = (m["version"] as? Long) ?: throw IOException("key file: no version")
            if (version != 1L) throw UnsupportedKeyFileException("key file version $version (this app knows 1): update Hoppy")
            val secret = (m["x25519_private"] as? String)?.let { Crypto.unhex32(it) }
                ?: throw IOException("key file: invalid private key")
            return DeviceKey(secret)
        }

        /**
         * Loads the key from [file] or creates it (owner-only where the file system allows).
         * A file that cannot be parsed is kept as `<name>.bad` (Rust: `with_extension("bad")`)
         * and replaced; a newer version is an error and is never overwritten. Two creators
         * end up with the same key: the file is published with an exclusive link.
         */
        @Synchronized
        fun loadOrCreate(file: File): DeviceKey {
            if (file.exists()) {
                try {
                    return fromJson(file.readText())
                } catch (e: UnsupportedKeyFileException) {
                    throw e
                } catch (e: IOException) {
                    file.renameTo(File(file.parentFile, file.nameWithoutExtension + ".bad"))
                }
            }
            val key = generate()
            file.parentFile?.mkdirs()
            val tmp = tempFile(file)
            writeOwnerOnly(tmp, key.toJson().toByteArray(Charsets.UTF_8))
            try {
                try {
                    Files.createLink(file.toPath(), tmp.toPath()) // fails if it exists: never replaces a key
                } catch (e: FileAlreadyExistsException) {
                    return fromJson(file.readText())
                } catch (e: UnsupportedOperationException) {
                    Files.move(tmp.toPath(), file.toPath()) // no REPLACE_EXISTING
                } catch (e: IOException) {
                    if (file.exists()) return fromJson(file.readText())
                    Files.move(tmp.toPath(), file.toPath())
                }
            } finally {
                tmp.delete()
            }
            return key
        }
    }
}

private fun tempFile(target: File): File =
    File(target.parentFile, ".${target.name}.${ProcessHandleCompat.pid()}.${Crypto.hex(Crypto.randomBytes(8))}.tmp")

/** Creates [file] (must not exist) readable by the owner only, writes [data] and syncs. */
internal fun writeOwnerOnly(file: File, data: ByteArray) {
    val path = file.toPath()
    try {
        Files.createFile(path, PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rw-------")))
    } catch (e: UnsupportedOperationException) {
        if (!file.createNewFile()) throw FileAlreadyExistsException(file.path)
        file.setReadable(false, false); file.setReadable(true, true)
        file.setWritable(false, false); file.setWritable(true, true)
    }
    java.io.FileOutputStream(file).use { out ->
        out.write(data)
        out.fd.sync()
    }
}

/** Atomically replaces [file] with [data] (owner-only temp file, then rename). */
internal fun writePrivate(file: File, data: ByteArray) {
    file.parentFile?.mkdirs()
    val tmp = tempFile(file)
    try {
        writeOwnerOnly(tmp, data)
        Files.move(tmp.toPath(), file.toPath(), StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
    } finally {
        tmp.delete()
    }
}

/** `ProcessHandle` is Java 9+ and missing on Android: a per-process random tag is enough. */
private object ProcessHandleCompat {
    private val tag = Crypto.hex(Crypto.randomBytes(4))
    fun pid(): String = tag
}
