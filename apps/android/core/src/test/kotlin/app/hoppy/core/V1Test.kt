package app.hoppy.core

import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.File
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketTimeoutException
import java.nio.file.Files
import kotlin.concurrent.thread
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNotEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** Protocol v1 (spec §7): every `v1` vector of docs/test-vectors.json, RFC KATs, sessions. */
@Suppress("UNCHECKED_CAST")
class V1Test {
    private fun vectors(): Map<String, Any?> {
        var dir: File? = File("").absoluteFile
        while (dir != null && !File(dir, "docs/test-vectors.json").exists()) dir = dir.parentFile
        return Json.decodeObject(File(dir!!, "docs/test-vectors.json").readText())
    }

    private val v1 by lazy { vectors()["v1"] as Map<String, Any?> }
    private fun obj(m: Any?, k: String) = (m as Map<String, Any?>)[k] as Map<String, Any?>
    private fun str(m: Any?, k: String) = (m as Map<String, Any?>)[k] as String
    private fun hex(s: String) = Crypto.unhex(s)!!
    private fun h(b: ByteArray) = Crypto.hex(b)
    private fun dev(m: Map<String, Any?>) = DeviceInfo(m["id"] as String, m["name"] as String, m["platform"] as String)

    // ------------------------------------------------------------------ primitives

    @Test
    fun rfcKnownAnswers() {
        // RFC 7748 §5.2 and §6.1
        assertEquals(
            "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552",
            h(Crypto.x25519(hex("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4"),
                hex("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c"))),
        )
        val alice = hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
        val bob = hex("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb")
        assertEquals("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a", h(Crypto.x25519Public(alice)))
        assertEquals("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742", h(Crypto.x25519(alice, Crypto.x25519Public(bob))))
        // RFC 8439 §2.8.2
        val key = ByteArray(32) { (0x80 + it).toByte() }
        val nonce = hex("070000004041424344454647")
        val aad = hex("50515253c0c1c2c3c4c5c6c7")
        val pt = ("Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, " +
            "sunscreen would be it.").toByteArray()
        val sealed = Crypto.aead(true, key, nonce, aad, pt)
        assertEquals("1ae10b594f09e26a7e902ecbd0600691", h(sealed.copyOfRange(sealed.size - 16, sealed.size)))
        assertEquals("d31a8d34648e60db7b86afbc53ef7ec2", h(sealed.copyOfRange(0, 16)))
        assertContentEquals(pt, Crypto.aead(false, key, nonce, aad, sealed))
        val bad = sealed.copyOf().also { it[it.size - 1] = (it[it.size - 1].toInt() xor 1).toByte() }
        assertFailsWith<CryptoException> { Crypto.aead(false, key, nonce, aad, bad) }
        // RFC 4231 case 2, RFC 5869 case 1, RFC 4648
        assertEquals("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            h(Crypto.hmacSha256("Jefe".toByteArray(), "what do ya want for nothing?".toByteArray())))
        assertEquals("3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865",
            h(Crypto.hkdf(ByteArray(13) { it.toByte() }, ByteArray(22) { 0x0b }, ByteArray(10) { (0xf0 + it).toByte() }, 42)))
        assertEquals("", Crypto.base32(ByteArray(0)))
        assertEquals("MY", Crypto.base32("f".toByteArray()))
        assertEquals("MZXW6YTBOI", Crypto.base32("foobar".toByteArray()))
        assertEquals(Crypto.FINGERPRINT_LEN, Crypto.fingerprint(ByteArray(0)).length)
    }

    @Test
    fun emojiTableMatchesVectors() {
        val rows = v1["sas_emoji"] as List<Map<String, Any?>>
        assertEquals(64, rows.size)
        assertEquals(64, Sas.TABLE.size)
        rows.forEachIndexed { i, row ->
            val ours = Sas.TABLE[i]
            assertEquals(i.toLong(), row["index"])
            assertEquals(row["emoji"], ours.emoji, "$i")
            assertEquals(row["it"], ours.it, "$i")
            assertEquals(row["en"], ours.en, "$i")
            assertEquals(1, ours.emoji.codePointCount(0, ours.emoji.length), "one code point: $i")
            assertEquals(row["codepoints"], "U+%04X".format(ours.emoji.codePointAt(0)))
        }
        assertEquals(64, Sas.TABLE.map { it.emoji }.toSet().size)
        assertEquals(64, Sas.TABLE.map { it.it }.toSet().size)
        assertEquals(64, Sas.TABLE.map { it.en }.toSet().size)
    }

    @Test
    fun sasRenderingMatchesVectors() {
        for (case in v1["sas_values"] as List<Map<String, Any?>>) {
            val sas = Sas.of((case["value"] as Long).toInt())
            assertEquals((case["emoji"] as List<Long>).map { it.toInt() }, sas.emoji)
            assertEquals(case["text"], sas.text)
        }
        assertEquals(0xFFFFFF, Sas.of(0x1FFFFFF).value)
        assertEquals(List(4) { "elefante" }, Sas.of(0).words(true))
        val s = obj(v1, "sas")
        val sas = Sas.of((s["value"] as Long).toInt())
        assertEquals(s["it"], sas.words(true))
        assertEquals(s["en"], sas.words(false))
    }

    @Test
    fun keysAndFingerprintsMatchVectors() {
        for (who in listOf("initiator", "responder")) {
            val k = obj(obj(v1, "keys"), who)
            val key = DeviceKey.fromSecret(hex(k["static_private"] as String))
            assertEquals(k["static_public"], h(key.public))
            assertEquals(k["fingerprint"], key.fingerprint)
            assertTrue(Crypto.isFingerprint(key.fingerprint))
            assertContentEquals(key.public, DeviceKey.fromJson(key.toJson()).public)
            assertFalse(Crypto.isLowOrder(key.public))
        }
        for (p in obj(v1, "negative")["low_order_points"] as List<String>) assertTrue(Crypto.isLowOrder(hex(p)), p)
        assertTrue(Crypto.isLowOrder(ByteArray(31) { 1 }), "wrong length")
    }

    @Test
    fun keyFileIsStableAndNeverOverwritten() {
        val dir = Files.createTempDirectory("key").toFile()
        val file = File(dir, "sub/identity.key")
        val a = DeviceKey.loadOrCreate(file)
        assertContentEquals(a.public, DeviceKey.loadOrCreate(file).public)
        val mode = Files.getPosixFilePermissions(file.toPath())
        assertEquals(setOf(java.nio.file.attribute.PosixFilePermission.OWNER_READ, java.nio.file.attribute.PosixFilePermission.OWNER_WRITE), mode)
        file.writeText("garbage")
        val c = DeviceKey.loadOrCreate(file)
        assertFalse(c.public.contentEquals(a.public))
        assertTrue(File(file.parentFile, "identity.bad").exists())
        file.writeText("""{"version":2,"x25519_private":"00"}""")
        assertFailsWith<UnsupportedKeyFileException> { DeviceKey.loadOrCreate(file) }
        assertTrue(file.readText().contains("\"version\":2"))
        assertTrue(file.parentFile.listFiles()!!.none { it.name.endsWith(".tmp") }, "no temp files left")
    }

    // ------------------------------------------------------------------ handshake vectors

    private class Pair2(val i: Initiator, val r: Responder)

    private fun vectorPair(afterMsg1: Boolean = true): Pair2 {
        val ki = obj(obj(v1, "keys"), "initiator")
        val kr = obj(obj(v1, "keys"), "responder")
        val i = Initiator.forTest(DeviceKey.fromSecret(hex(str(ki, "static_private"))), hex(str(ki, "ephemeral_private")), hex(str(ki, "sas_nonce")))
        val r = Responder.forTest(DeviceKey.fromSecret(hex(str(kr, "static_private"))), hex(str(kr, "ephemeral_private")), hex(str(kr, "sas_nonce")))
        if (afterMsg1) r.readMsg1(i.writeMsg1())
        return Pair2(i, r)
    }

    @Test
    fun handshakeMatchesVectors() {
        val hs = obj(v1, "handshake")
        val ki = obj(obj(v1, "keys"), "initiator")
        val kr = obj(obj(v1, "keys"), "responder")
        assertEquals(hs["prologue_hex"], h(Noise.PROLOGUE))
        assertEquals(hs["protocol_name"], Noise.PROTOCOL_NAME)
        val p = vectorPair(afterMsg1 = false)
        val m1 = p.i.writeMsg1()
        assertEquals(hs["msg1_hex"], h(m1))
        p.r.readMsg1(m1)
        val m2 = p.r.writeMsg2(dev(obj(kr, "device")))
        assertEquals(hs["msg2_hex"], h(m2))
        assertEquals(hs["h2_hex"], h(p.r.handshakeHash))
        val seenByI = p.i.readMsg2(m2)
        assertEquals(dev(obj(kr, "device")), seenByI.device)
        assertEquals(kr["fingerprint"], seenByI.fingerprint)
        val m3 = p.i.writeMsg3(dev(obj(ki, "device")))
        assertEquals(hs["msg3_hex"], h(m3))
        val seenByR = p.r.readMsg3(m3)
        assertEquals(ki["fingerprint"], seenByR.fingerprint)
        assertEquals(hs["h_hex"], h(p.i.handshakeHash))
        assertEquals(hs["h_hex"], h(p.r.handshakeHash))
        val sas = obj(v1, "sas")
        assertEquals(seenByI.sas, seenByR.sas)
        assertEquals((sas["value"] as Long).toInt(), seenByI.sas.value)
        assertEquals(sas["text"], seenByI.sas.text)

        val (iTx, iRx) = p.i.finish()
        val (rTx, rRx) = p.r.finish()
        for (t in v1["transport"] as List<Map<String, Any?>>) {
            val plain = hex(t["plaintext_hex"] as String)
            val fromI = t["from"] == "initiator"
            val sealed = (if (fromI) iTx else rTx).encrypt(ByteArray(0), plain)
            val framed = ByteArrayOutputStream().also { Noise.writeFrame(it, sealed) }.toByteArray()
            assertEquals(t["message_hex"], h(framed))
            val rx = if (fromI) rRx else iRx
            val tampered = sealed.copyOf().also { it[0] = (it[0].toInt() xor 1).toByte() }
            assertFailsWith<CryptoException> { rx.decrypt(ByteArray(0), tampered) }
            // A failed open does not move the counter: the real message still opens.
            assertContentEquals(plain, rx.decrypt(ByteArray(0), sealed))
        }
    }

    @Test
    fun transportCountersRejectReplay() {
        val p = vectorPair()
        p.i.readMsg2(p.r.writeMsg2(DeviceInfo("r", "R", "linux")))
        p.r.readMsg3(p.i.writeMsg3(DeviceInfo("i", "I", "linux")))
        val (iTx, _) = p.i.finish()
        val (_, rRx) = p.r.finish()
        val c0 = iTx.encrypt(ByteArray(0), "hello".toByteArray())
        val c1 = iTx.encrypt(ByteArray(0), "again".toByteArray())
        assertFailsWith<CryptoException>("reordered") { vectorFreshRx().decrypt(ByteArray(0), c1) }
        assertContentEquals("hello".toByteArray(), rRx.decrypt(ByteArray(0), c0))
        assertFailsWith<CryptoException>("replayed") { rRx.decrypt(ByteArray(0), c0) }
    }

    private fun vectorFreshRx(): CipherState {
        val p = vectorPair()
        p.i.readMsg2(p.r.writeMsg2(DeviceInfo("r", "R", "linux")))
        p.r.readMsg3(p.i.writeMsg3(DeviceInfo("i", "I", "linux")))
        return p.r.finish().second
    }

    @Test
    fun negativeVectorsAreRejected() {
        val neg = obj(v1, "negative")
        val respDev = dev(obj(obj(obj(v1, "keys"), "responder"), "device"))
        assertFailsWith<CryptoException> { Responder(DeviceKey.generate()).readMsg1(hex(str(neg, "msg1_low_order_hex"))) }

        var p = vectorPair()
        p.r.writeMsg2(respDev)
        assertFailsWith<CryptoException> { p.i.readMsg2(hex(str(neg, "msg2_tampered_hex"))) }
        for (case in listOf("msg3_tampered_hex", "msg3_bad_commitment_hex")) {
            p = vectorPair()
            p.i.readMsg2(p.r.writeMsg2(respDev))
            assertFailsWith<CryptoException>(case) { p.r.readMsg3(hex(str(neg, case))) }
        }
        val long = hex(str(neg, "handshake_frame_too_long_hex")) + ByteArray(3000)
        assertFailsWith<ProtocolException> { Noise.readFrame(ByteArrayInputStream(long), Noise.MAX_HANDSHAKE_MESSAGE) }
        // Short messages are refused before any crypto.
        assertFailsWith<CryptoException> { vectorPair().i.readMsg2(ByteArray(Noise.MIN_MSG2 - 1)) }

        p = vectorPair()
        p.i.readMsg2(p.r.writeMsg2(respDev))
        p.r.readMsg3(p.i.writeMsg3(dev(obj(obj(obj(v1, "keys"), "initiator"), "device"))))
        val rx = p.r.finish().second
        for (case in listOf("transport_empty_hex", "transport_short_hex")) {
            assertFailsWith<ProtocolException>(case) { NoiseInputStream(ByteArrayInputStream(hex(str(neg, case))), rx).read() }
        }
        assertContentEquals(byteArrayOf(0, 0), hex(str(neg, "busy_reply_hex")))
    }

    @Test
    fun handshakeNamesAreCleanedAndIdsChecked() {
        val nonce = "00".repeat(32)
        val ok = """{"id":"x","name":"Ev‮il\u0007 ${"n".repeat(100)}","platform":"pla​tform-too-long-x","nonce":"$nonce"}"""
        val (d, _) = parseIdentity(ok.toByteArray())
        assertEquals(Noise.MAX_NAME_CHARS, d.name.codePointCount(0, d.name.length))
        assertTrue(d.name.startsWith("Evil "))
        assertEquals(Noise.MAX_PLATFORM_CHARS, d.platform.length)
        for (badId in listOf("\"x\\u202e\"", "\" x\"", "\"\"", "1")) {
            assertFailsWith<ProtocolException>(badId) {
                parseIdentity("""{"id":$badId,"name":"n","platform":"p","nonce":"$nonce"}""".toByteArray())
            }
        }
        assertFailsWith<ProtocolException> { parseIdentity("""{"id":"x","name":1,"platform":"p","nonce":"$nonce"}""".toByteArray()) }
        assertEquals("ab‍", Text.display("a‮b‍\u0007 ", 10))
    }

    @Test
    fun streamRoundTripSpansMessages() {
        val p = vectorPair()
        p.i.readMsg2(p.r.writeMsg2(DeviceInfo("r", "R", "linux")))
        p.r.readMsg3(p.i.writeMsg3(DeviceInfo("i", "I", "linux")))
        val (iTx, _) = p.i.finish()
        val (_, rRx) = p.r.finish()
        val data = ByteArray(200_000) { (it % 251).toByte() }
        val wire = ByteArrayOutputStream()
        val w = NoiseOutputStream(wire, iTx)
        w.write(data)
        w.flush()
        w.flush() // nothing buffered: no empty message
        assertEquals(data.size + 4 * (2 + Crypto.TAG_LEN), wire.size(), "4 messages")
        assertContentEquals(data, NoiseInputStream(ByteArrayInputStream(wire.toByteArray()), rRx).readBytes())
        val cut = wire.toByteArray().copyOf(wire.size() - 1)
        assertFailsWith<java.io.IOException> { NoiseInputStream(ByteArrayInputStream(cut), vectorFreshRx()).readBytes() }
    }

    // ------------------------------------------------------------------ wire extras

    @Test
    fun discoveryAndCallbackVectors() {
        val d = obj(v1, "discovery")
        // `crypto` is read leniently, the same way in every implementation.
        @Suppress("UNCHECKED_CAST")
        for (case in d["lenient_crypto"] as List<Map<String, Any?>>) {
            val text = """{"hoppy":0,"type":"announce","id":"x","name":"n","platform":"linux","port":1,"crypto":${Json.encode(case["value"])}}"""
            val msg = DiscoveryMessage.decode(text.toByteArray())
            assertNotNull(msg, text)
            assertEquals((case["crypto"] as Long?)?.toInt(), msg.crypto, text)
            assertEquals((case["crypto"] as Long?)?.toInt(), Protocol.lenientCrypto(case["value"]), text)
        }
        val ann = DiscoveryMessage.decode(str(d, "announce_json").toByteArray())!!
        assertEquals(1, ann.crypto)
        assertEquals(str(d, "announce_json"), String(ann.encode()))
        val cb = DiscoveryMessage.decode(str(d, "callback_json").toByteArray())!!
        assertEquals(str(d, "callback_json"), String(cb.encode()))
        val hello = Json.decodeObject(str(d, "callback_hello_json"))
        val out = ByteArrayOutputStream()
        Protocol.writeCallbackHello(out, hello["id"] as String, hello["token"] as String, crypto = true)
        assertEquals(str(d, "callback_hello_hex"), h(out.toByteArray()))
        assertEquals(1, Protocol.readCallbackHello(ByteArrayInputStream(out.toByteArray())).crypto)
        // `crypto` of an unexpected type or size never hides the peer.
        val base = """{"hoppy":0,"type":"announce","id":"x","name":"n","platform":"linux","port":1,"""
        for ((raw, want) in listOf(
            "\"crypto\":\"1\"" to null, "\"crypto\":-1" to null, "\"crypto\":1.5" to null, "\"crypto\":null" to null,
            "\"crypto\":{\"v\":2}" to null, "\"crypto\":300" to 255, "\"crypto\":2" to 2, "\"crypto\":true" to null,
        )) {
            val msg = DiscoveryMessage.decode("$base$raw}".toByteArray())
            assertEquals(want, msg!!.crypto, raw)
        }
    }

    @Test
    fun versionPlan() {
        val p = TransferClient.Plan.ENCRYPTED
        assertEquals(p, TransferClient.plan(1, false, true))
        assertEquals(p, TransferClient.plan(2, false, true), "future versions")
        assertEquals(TransferClient.Plan.PLAINTEXT, TransferClient.plan(0, false, true), "old app")
        assertEquals(p, TransferClient.plan(0, true, true), "known key: no downgrade")
        assertEquals(TransferClient.Plan.ENCRYPTED_OR_FALLBACK, TransferClient.plan(null, false, true), "typed address")
        assertEquals(p, TransferClient.plan(null, true, true))
        assertEquals(p, TransferClient.plan(0, false, false))
        assertEquals(p, TransferClient.plan(null, false, false))
    }

    // ------------------------------------------------------------------ trust and guard

    private fun peer(id: String, fp: String) = PeerKey(id, "name-$id", "linux", fp)

    @Test
    fun tofuStates() {
        val s = TrustStore()
        val a = peer("a", "FP1")
        assertEquals(TrustState.NEW, s.state("a", "FP1"))
        assertTrue(s.pin(a, null, 10))
        assertFalse(s.pin(a, null, 11))
        assertEquals(TrustState.KNOWN, s.state("a", "FP1"))
        assertEquals(TrustState.KEY_CHANGED, s.state("a", "FP2"))
        assertFalse(s.pin(peer("a", "FP2"), null, 12), "pin never replaces a key")
        s.setVerified(a, 13)
        assertEquals(TrustState.VERIFIED, s.state("a", "FP1"))
        assertEquals(10L, s.peers["a"]!!.firstSeen)
        assertFailsWith<KeyChangedException> { s.setVerified(peer("a", "FP2"), 14) }
        s.retrust(peer("a", "FP2"), 15)
        assertEquals(TrustState.KNOWN, s.state("a", "FP2"))
        assertTrue(s.forget("a"))
        assertFalse(s.forget("a"))
        s.setVerified(peer("b", "X"), 1)
        assertEquals(TrustState.VERIFIED, s.state("b", "X"), "verify pins on first contact")
        s.pin(peer("c", "Y"), "192.168.1.5:47471", 2)
        val back = TrustStore.fromJson(s.toJson())
        assertEquals(s.peers, back.peers)
        assertEquals(1, TrustStore.fromJson("{}").version)
        assertTrue(back.encryptedAt("192.168.1.5:47471"))
        assertFalse(back.encryptedAt("192.168.1.5:47472"))
        assertTrue(back.nameTakenByOther("z", " NAME-C "))
        assertFalse(back.nameTakenByOther("c", "name-c"), "its own name")
        // Rust writes this shape (pretty-printed); we read it.
        val rust = """{"version": 1, "peers": {"pc": {"fingerprint": "F", "name": "PC", "first_seen": 5, "verified": true, "addr": "[fe80::1]:47471"}}}"""
        assertEquals(TrustEntry("F", "PC", 5, true, "[fe80::1]:47471"), TrustStore.fromJson(rust).peers["pc"])
    }

    @Test
    fun fileStoreIsSharedAndDamageIsNotFatal() {
        val dir = Files.createTempDirectory("trust").toFile()
        val file = File(dir, "trust.json")
        val one = Trust.open(file)
        val two = Trust.open(file)
        one.pin(peer("a", "FP"))
        Thread.sleep(5)
        assertEquals(TrustState.KNOWN, two.state("a", "FP"), "re-read before use")
        two.markVerified(peer("a", "FP"))
        assertEquals(TrustState.VERIFIED, one.state("a", "FP"))
        assertTrue(one.forget("a"))
        assertEquals(TrustState.NEW, two.state("a", "FP"))
        val handles = (0 until 8).map { i -> Trust.open(file).let { t -> thread { t.pin(peer("p$i", "FP")) } } }
        handles.forEach { it.join() }
        assertEquals(8, Trust.open(file).snapshot().peers.size)
        assertFalse(File(dir, "trust.json.lock").exists())
        file.writeText("not json")
        val t = Trust.open(file)
        assertTrue(t.snapshot().peers.isEmpty())
        assertTrue(t.takeWarning()!!.contains("damaged"))
        assertNull(t.takeWarning())
        assertTrue(File(dir, "trust.json.bad").exists())
        t.pin(peer("a", "FP"))
        assertEquals(TrustState.KNOWN, Trust.open(file).state("a", "FP"))
        // A stale lock (crashed process) does not block.
        File(dir, "trust.json.lock").apply { writeText(""); setLastModified(System.currentTimeMillis() - 60_000) }
        t.pin(peer("b", "FP"))
    }

    @Test
    fun guardCapsAbortsPerIpAndInTotal() {
        val g = HandshakeGuard()
        val a = "192.168.1.9"
        repeat(HandshakeGuard.MAX_ABORTS_PER_IP) { i ->
            assertNull(g.admit(a, 0), "$i")
            g.recordAbort(a, 0)
        }
        assertTrue(g.admit(a, 0) != null)
        assertNull(g.admit("192.168.1.10", 0), "others still served")
        assertTrue(g.admit(a, 59_000) != null)
        assertNull(g.admit(a, 61_000))
        repeat(HandshakeGuard.MAX_ABORTS_PER_IP) { g.recordAbort(a, 61_000) }
        assertTrue(g.admit(a, 161_000) != null, "second strike: 120 s")
        assertNull(g.admit(a, 182_000))

        val t = HandshakeGuard()
        repeat(HandshakeGuard.MAX_ABORTS_TOTAL) { i -> assertNull(t.admit("10.0.0.$i", 0)); t.recordAbort("10.0.0.$i", 0) }
        assertTrue(t.admit("10.0.1.1", 0) != null, "global cap")
        assertNull(t.admit("10.0.1.1", 60_000))
        var tries = 0
        var now = 60_000L
        var n = 0
        while (now < 190_000L) {
            n++
            val src = "10.1.${n shr 8}.${n and 255}"
            if (t.admit(src, now) == null) { t.recordAbort(src, now); tries++ }
            now += 100
        }
        assertTrue(tries <= 3 * HandshakeGuard.MAX_ABORTS_TOTAL, "$tries")

        assertTrue(t.shouldWarn("k", 0))
        assertFalse(t.shouldWarn("k", 10_000))
        assertTrue(t.shouldWarn("other", 0))
        assertTrue(t.shouldWarn("k", HandshakeGuard.WARNING_INTERVAL_MS))
    }

    // ------------------------------------------------------------------ sessions over TCP

    private val me = DeviceInfo("tx-id", "Phone", "android")
    private val rxDev = DeviceInfo("rx-id", "PC", "windows")

    private class Rec(val accept: Boolean = true, val auto: Boolean = false) : ReceiveHandler {
        @Volatile var session: SessionInfo? = null
        @Volatile var asked = 0
        @Volatile var error: String? = null
        val blocked = java.util.concurrent.CopyOnWriteArrayList<String>()
        val warnings = java.util.concurrent.CopyOnWriteArrayList<Warning>()
        override fun decide(offer: Offer, from: InetAddress, session: SessionInfo): Boolean {
            asked++
            this.session = session
            return accept
        }
        override fun autoAccepts() = auto
        override fun onFinished(from: InetAddress, offer: Offer, saved: List<String>, error: String?) { this.error = error }
        override fun onBlocked(from: InetAddress, offer: Offer, session: SessionInfo, reason: String) { blocked += reason }
        override fun onWarning(from: InetAddress, warning: Warning) { warnings += warning }
    }

    /** A receiver serving connections until the test ends. */
    private fun server(rx: Local, handler: ReceiveHandler, dir: File = Files.createTempDirectory("dst").toFile()): Int {
        val ss = ServerSocket(0, 50, InetAddress.getLoopbackAddress())
        thread(isDaemon = true) {
            while (true) {
                val c = runCatching { ss.accept() }.getOrNull() ?: break
                thread(isDaemon = true) { TransferServer.handle(c, DirectorySink(dir), handler, rx) }
            }
        }
        return ss.localPort
    }

    private fun file(text: String = "hello"): FileSource {
        val f = File(Files.createTempDirectory("src").toFile(), "a.txt")
        f.writeText(text)
        return FileSource(f)
    }

    private fun peerAt(port: Int, crypto: Int? = 1, id: String = rxDev.id) =
        Peer(DeviceInfo(id, rxDev.name, rxDev.platform), InetAddress.getLoopbackAddress(), port, crypto)

    private fun waitFor(ms: Long = 2_000, cond: () -> Boolean) {
        val end = System.currentTimeMillis() + ms
        while (!cond() && System.currentTimeMillis() < end) Thread.sleep(10)
    }

    private class Seen : SendListener {
        @Volatile var info: SessionInfo? = null
        override fun onSession(info: SessionInfo) { this.info = info }
    }

    @Test
    fun keyChangedBlocksBothSides() {
        val tx = Local.ephemeral(me)
        val rx = Local.ephemeral(rxDev)
        val rec = Rec()
        val port = server(rx, rec)
        assertEquals(SendOutcome.Delivered, TransferClient.sendToPeer(peerAt(port), tx, listOf(file()), null))
        assertEquals(TrustState.KNOWN, tx.trust.state(rxDev.id, rx.fingerprint))
        // Second contact: Known, same code on both ends.
        val seen = Seen()
        TransferClient.sendToPeer(peerAt(port), tx, listOf(file()), null, seen)
        assertEquals(TrustState.KNOWN, seen.info!!.trust)
        assertEquals(TrustState.KNOWN, rec.session!!.trust)

        // The receiver is "reinstalled" (new key, same id): the sender refuses before message 3.
        val rx2 = Local.ephemeral(rxDev)
        val rec2 = Rec()
        val port2 = server(rx2, rec2)
        val e = assertFailsWith<KeyChangedException> { TransferClient.sendToPeer(peerAt(port2), tx, listOf(file()), null) }
        assertEquals(rx2.fingerprint, e.peer.fingerprint)
        assertEquals(0, rec2.asked, "no offer sent")
        // "Forget device": the next contact is New and works.
        assertTrue(tx.trust.forget(rxDev.id))
        val again = Seen()
        assertEquals(SendOutcome.Delivered, TransferClient.sendToPeer(peerAt(port2), tx, listOf(file()), null, again))
        assertEquals(TrustState.NEW, again.info!!.trust)

        // Receiver side: a sender with another key under a remembered id is declined unasked.
        val impostor = Local.ephemeral(me)
        val asked = rec.asked
        val out = TransferClient.sendToPeer(peerAt(port), impostor, listOf(file()), null)
        assertEquals(SendOutcome.Declined("declined"), out, "the specific reason stays local")
        assertEquals(asked, rec.asked)
        waitFor { rec.blocked.isNotEmpty() } // reported right after the decline went out
        assertEquals(listOf("key_changed"), rec.blocked.toList())
        TransferClient.sendToPeer(peerAt(port), impostor, listOf(file()), null)
        Thread.sleep(100)
        assertEquals(1, rec.blocked.size, "warnings are rate-limited")
    }

    @Test
    fun autoAcceptDoesNotPinAndNameCollisionsAreFlagged() {
        val tx = Local.ephemeral(me)
        val rx = Local.ephemeral(rxDev)
        val rec = Rec(auto = true)
        val port = server(rx, rec)
        TransferClient.sendToPeer(peerAt(port), tx, listOf(file()), null)
        assertNull(rx.trust.get(me.id), "TOFU needs a person's yes")
        assertEquals(TrustState.KNOWN, tx.trust.state(rxDev.id, rx.fingerprint))
        // Another device calling itself "PC" under a new id: New + name_matches_known.
        val other = Local.ephemeral(DeviceInfo("other-id", "pc ", "linux"))
        val seen = Seen()
        TransferClient.sendToPeer(peerAt(server(other, Rec()), id = "other-id"), tx, listOf(file()), null, seen)
        assertEquals(TrustState.NEW, seen.info!!.trust)
        assertTrue(seen.info!!.nameMatchesKnown)
    }

    @Test
    fun plaintextFromAKnownDeviceIsRefused() {
        val tx = Local.ephemeral(me)
        val rx = Local.ephemeral(rxDev)
        val rec = Rec()
        val port = server(rx, rec)
        TransferClient.sendToPeer(peerAt(port), tx, listOf(file()), null)
        assertEquals(TrustState.KNOWN, rx.trust.state(me.id, tx.fingerprint))
        // A v0 offer carrying the remembered id (downgrade or impersonation).
        Socket(InetAddress.getLoopbackAddress(), port).use { s ->
            Protocol.writeHello(s.getOutputStream())
            Protocol.writeJson(s.getOutputStream(), Offer(me, listOf(FileEntry("x", 1))).toMap())
            assertEquals("declined", Protocol.readJson(s.getInputStream())["reason"])
        }
        waitFor { rec.blocked.isNotEmpty() }
        assertEquals(listOf("encryption_required"), rec.blocked.toList())
        // A strict receiver refuses every plaintext offer.
        val strict = Local(rxDev, DeviceKey.generate(), Trust.inMemory(), allowPlaintext = false)
        val sp = server(strict, Rec())
        Socket(InetAddress.getLoopbackAddress(), sp).use { s ->
            Protocol.writeHello(s.getOutputStream())
            Protocol.writeJson(s.getOutputStream(), Offer(me, listOf(FileEntry("x", 1))).toMap())
            assertEquals("encryption_required", Protocol.readJson(s.getInputStream())["reason"])
        }
        // ... and a strict sender never speaks v0, even to a v0 announce.
        val strictTx = Local(me, DeviceKey.generate(), Trust.inMemory(), allowPlaintext = false)
        val seen = Seen()
        TransferClient.sendToPeer(peerAt(server(Local.ephemeral(rxDev), Rec()), crypto = 0), strictTx, listOf(file()), null, seen)
        assertTrue(seen.info!!.encrypted)
    }

    /** A v0 node: reads the hello, closes on version 1, serves v0 (old Android/desktop app). */
    private fun v0Server(dir: File): Int {
        val ss = ServerSocket(0, 50, InetAddress.getLoopbackAddress())
        thread(isDaemon = true) {
            while (true) {
                val c = runCatching { ss.accept() }.getOrNull() ?: break
                thread(isDaemon = true) {
                    c.use { s ->
                        runCatching {
                            val input = s.getInputStream()
                            Protocol.readHello(input) // throws on HOPY 0x01: the socket closes
                            val offer = Offer.from(Protocol.readJson(input))
                            Protocol.writeJson(s.getOutputStream(), mapOf("accept" to true))
                            for (f in offer.files) {
                                val data = ByteArray(f.size.toInt()).also { java.io.DataInputStream(input).readFully(it) }
                                java.io.DataInputStream(input).readFully(ByteArray(32))
                                File(dir, f.name).writeBytes(data)
                            }
                            Protocol.writeJson(s.getOutputStream(), mapOf("ok" to true))
                        }
                    }
                }
            }
        }
        return ss.localPort
    }

    @Test
    fun typedAddressFallsBackToV0OnlyWhenNothingIsKnown() {
        val dir = Files.createTempDirectory("v0").toFile()
        val port = v0Server(dir)
        val tx = Local.ephemeral(me)
        val seen = Seen()
        assertEquals(SendOutcome.Delivered, TransferClient.send(InetAddress.getLoopbackAddress(), port, tx, listOf(file("v0!")), seen))
        assertFalse(seen.info!!.encrypted, "flagged not encrypted")
        assertEquals("v0!", File(dir, "a.txt").readText())
        // A discovered v0 peer (crypto 0): plaintext straight away.
        val seen0 = Seen()
        TransferClient.sendToPeer(peerAt(port, crypto = 0), tx, listOf(file()), null, seen0)
        assertFalse(seen0.info!!.encrypted)
        // It advertised crypto: never fall back.
        assertFailsWith<ProtocolException> { TransferClient.sendToPeer(peerAt(port, crypto = 1), tx, listOf(file()), null) }
        // A remembered device encrypted at this address before: never fall back (downgrade).
        tx.trust.pin(PeerKey("somebody", "S", "linux", "FP"), addrKey(InetAddress.getLoopbackAddress(), port))
        val e = assertFailsWith<UnencryptedException> { TransferClient.send(InetAddress.getLoopbackAddress(), port, tx, listOf(file())) }
        assertTrue(e.message!!.contains("downgrade"))
    }

    @Test
    fun abortedHandshakesAreLimitedAndBusyNeverFallsBack() {
        val rx = Local.ephemeral(rxDev)
        val rec = Rec()
        val port = server(rx, rec)
        var served = 0
        var refused = 0
        repeat(7) {
            Socket(InetAddress.getLoopbackAddress(), port).use { s ->
                s.soTimeout = 5_000
                val out = s.getOutputStream()
                out.write(Noise.PROLOGUE)
                Noise.writeFrame(out, Initiator(DeviceKey.generate()).writeMsg1())
                val msg2 = Noise.readFrame(s.getInputStream(), Noise.MAX_HANDSHAKE_MESSAGE)
                if (msg2 != null && msg2.isNotEmpty()) served++ else if (msg2 != null) refused++
            }
            Thread.sleep(60) // the receiver records the abort after our close
        }
        assertEquals(5 to 2, served to refused)
        waitFor { rec.warnings.isNotEmpty() }
        assertTrue(rec.warnings.any { it is Warning.HandshakeAborts }, "the user is warned")
        assertEquals(1, rec.warnings.count { it is Warning.HandshakeAborts }, "once a minute per source")
        // A real sender from the blocked IP gets "busy" — never a plaintext fallback, even typed.
        assertFailsWith<ReceiverBusyException> {
            TransferClient.send(InetAddress.getLoopbackAddress(), port, Local.ephemeral(me), listOf(file()))
        }
        assertEquals(0, rec.asked)
    }

    @Test
    fun senderDeadlineCoversHandshakeAndDecision() {
        // A receiver that completes the handshake but never answers the offer.
        val ss = ServerSocket(0, 5, InetAddress.getLoopbackAddress())
        val rxKey = DeviceKey.generate()
        thread(isDaemon = true) {
            ss.accept().use { s ->
                val input = s.getInputStream()
                Protocol.readHelloVersion(input)
                val r = Responder(rxKey)
                r.readMsg1(Noise.readFrame(input, 2048)!!)
                Noise.writeFrame(s.getOutputStream(), r.writeMsg2(rxDev))
                Noise.readFrame(input, 2048)
                Thread.sleep(5_000)
            }
        }
        val tx = Local(me, DeviceKey.generate(), Trust.inMemory(), sessionDeadlineMs = 700)
        val t0 = System.currentTimeMillis()
        assertFailsWith<SocketTimeoutException> {
            TransferClient.sendToPeer(peerAt(ss.localPort), tx, listOf(file()), null)
        }
        assertTrue(System.currentTimeMillis() - t0 < 3_000)
    }

    @Test
    fun offerMustComeFromTheHandshakeIdAndEmptyFramesEndSessions() {
        val rx = Local.ephemeral(rxDev)
        val rec = Rec()
        val port = server(rx, rec)
        val key = DeviceKey.generate()
        fun handshake(s: Socket): Pair<CipherState, CipherState> {
            val out = s.getOutputStream()
            val input = s.getInputStream()
            val i = Initiator(key)
            out.write(Noise.PROLOGUE)
            Noise.writeFrame(out, i.writeMsg1())
            i.readMsg2(Noise.readFrame(input, 2048)!!)
            Noise.writeFrame(out, i.writeMsg3(me))
            return i.finish()
        }
        Socket(InetAddress.getLoopbackAddress(), port).use { s ->
            val (txc, rxc) = handshake(s)
            val w = NoiseOutputStream(s.getOutputStream(), txc)
            Protocol.writeJson(w, Offer(DeviceInfo("someone-else", "X", "linux"), listOf(FileEntry("a", 1))).toMap())
            assertEquals("invalid_offer", Protocol.readJson(NoiseInputStream(s.getInputStream(), rxc))["reason"])
        }
        // Any first frame that is not an Offer (e.g. the old {"verify":…}) is an invalid offer (spec §7.4).
        Socket(InetAddress.getLoopbackAddress(), port).use { s ->
            val (txc, rxc) = handshake(s)
            val w = NoiseOutputStream(s.getOutputStream(), txc)
            Protocol.writeJson(w, mapOf("verify" to "0".repeat(32)))
            w.flush()
            assertEquals("invalid_offer", Protocol.readJson(NoiseInputStream(s.getInputStream(), rxc))["reason"])
        }
        assertEquals(TrustState.NEW, rx.trust.state(me.id, key.fingerprint), "nothing verified")
        Socket(InetAddress.getLoopbackAddress(), port).use { s ->
            s.soTimeout = 5_000
            handshake(s)
            Noise.writeFrame(s.getOutputStream(), txcEmpty())
            assertEquals(-1, s.getInputStream().read(), "an empty transport message ends the session")
        }
        assertEquals(0, rec.asked)
    }

    /** 16 bytes: an "empty" transport message (tag only, as an attacker could send). */
    private fun txcEmpty() = ByteArray(Crypto.TAG_LEN)

    @Test
    fun verificationIsLocalOnly() {
        // "Coincide" marks the other device verified on this device only: nothing is sent (spec §7.6).
        val tx = Local.ephemeral(me)
        val rx = Local.ephemeral(rxDev)
        val rec = Rec()
        val port = server(rx, rec)
        assertEquals(SendOutcome.Delivered, TransferClient.sendToPeer(peerAt(port), tx, listOf(file()), null))
        waitFor { rx.trust.get(me.id) != null }
        tx.trust.markVerified(PeerKey(rxDev.id, rxDev.name, rxDev.platform, rx.fingerprint))
        assertEquals(TrustState.VERIFIED, tx.trust.state(rxDev.id, rx.fingerprint))
        assertEquals(TrustState.KNOWN, rx.trust.state(me.id, tx.fingerprint))
    }
}
