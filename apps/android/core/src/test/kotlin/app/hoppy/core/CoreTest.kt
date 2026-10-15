package app.hoppy.core

import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.File
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.nio.file.Files
import java.security.MessageDigest
import kotlin.concurrent.thread
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class CoreTest {
    private val me = DeviceInfo("id-test", "Test", "android")

    /** Shared vectors live at the repo root: docs/test-vectors.json. */
    private fun vectors(): Map<String, Any?> {
        var dir: File? = File("").absoluteFile
        while (dir != null && !File(dir, "docs/test-vectors.json").exists()) dir = dir.parentFile
        return Json.decodeObject(File(dir!!, "docs/test-vectors.json").readText())
    }

    @Test
    fun sanitizeMatchesSharedVectors() {
        @Suppress("UNCHECKED_CAST")
        val cases = vectors()["sanitize"] as List<Map<String, Any?>>
        assertTrue(cases.isNotEmpty())
        for (c in cases) assertEquals(c["output"] as String?, FileNames.sanitize(c["input"] as String), "input ${c["input"]}")
    }

    @Test
    fun longNamesKeepExtension() {
        val out = FileNames.sanitize("🐰".repeat(300) + ".jpeg")!!
        assertEquals(200, out.codePointCount(0, out.length))
        assertTrue(out.endsWith(".jpeg"))
    }

    @Test
    fun offerExampleMatchesVectorBytes() {
        @Suppress("UNCHECKED_CAST")
        val ex = vectors()["offer_example"] as Map<String, Any?>
        val offer = Offer.from(Json.decodeObject(ex["offer_json"] as String))
        assertEquals(Json.decodeObject(ex["offer_json"] as String), Json.decodeObject(Json.encode(offer.toMap())))
        val sha = MessageDigest.getInstance("SHA-256").digest("hello".toByteArray())
        @Suppress("UNCHECKED_CAST")
        assertEquals((ex["sha256_hex"] as Map<String, Any?>)["a.txt"], sha.joinToString("") { "%02x".format(it) })
    }

    @Test
    fun jsonRoundTrip() {
        val v = mapOf("s" to "a\"b\\c\n🐰 è", "n" to 12345678901L, "b" to false, "z" to null, "l" to listOf(1L, "x"))
        assertEquals(v, Json.decode(Json.encode(v)))
        assertEquals(mapOf("u" to "é/"), Json.decode("""{"u":"é\/"}"""))
        assertFailsWith<JsonException> { Json.decode("{\"a\":1} x") }
        assertFailsWith<JsonException> { Json.decode("{\"a\":") }
    }

    @Test
    fun framesAndHello() {
        val out = ByteArrayOutputStream()
        Protocol.writeHello(out)
        Protocol.writeJson(out, mapOf("accept" to true))
        val bytes = out.toByteArray()
        assertContentEquals("HOPY".toByteArray() + 0, bytes.copyOfRange(0, 5))
        val input = ByteArrayInputStream(bytes)
        Protocol.readHello(input)
        assertEquals(true, Protocol.readJson(input)["accept"])
        assertFailsWith<ProtocolException> { Protocol.readHello(ByteArrayInputStream("HOPY\u0001".toByteArray())) }
        val huge = ByteArrayInputStream(byteArrayOf(0x7f, 0, 0, 0))
        assertFailsWith<ProtocolException> { Protocol.readJson(huge) }
    }

    @Test
    fun discoveryDecodeIsStrict() {
        val ok = DiscoveryMessage(DiscoveryMessage.ANNOUNCE, me, 47471)
        assertEquals(ok, DiscoveryMessage.decode(ok.encode()))
        val extra = """{"hoppy":0,"type":"announce","id":"x","name":"n","platform":"linux","port":1,"future":[1]}"""
        assertEquals(1, DiscoveryMessage.decode(extra.toByteArray())?.port)
        assertNull(DiscoveryMessage.decode("""{"hoppy":1,"type":"announce","id":"x","name":"n","platform":"l","port":1}""".toByteArray()))
        assertNull(DiscoveryMessage.decode("""{"hoppy":0,"type":"wave","id":"x","name":"n","platform":"l","port":1}""".toByteArray()))
        assertNull(DiscoveryMessage.decode("garbage".toByteArray()))
    }

    @Test
    fun callbackVectors() {
        @Suppress("UNCHECKED_CAST")
        val v = vectors()["callback_example"] as Map<String, Any?>
        val raw = v["datagram_json"] as String
        val msg = DiscoveryMessage.decode(raw.toByteArray())!!
        assertEquals(DiscoveryMessage.CALLBACK, msg.type)
        assertEquals(50123, msg.port)
        assertEquals(raw, String(msg.encode(), Charsets.UTF_8))
        @Suppress("UNCHECKED_CAST")
        for (bad in v["invalid_datagrams"] as List<String>) assertNull(DiscoveryMessage.decode(bad.toByteArray()), bad)
        val hello = Json.decodeObject(v["hello_json"] as String)
        val out = ByteArrayOutputStream()
        Protocol.writeCallbackHello(out, hello["id"] as String, hello["token"] as String, crypto = false)
        assertEquals(v["hello_hex"], out.toByteArray().joinToString("") { "%02x".format(it) })
        assertEquals(
            CallbackHello(hello["id"] as String, hello["token"] as String, null),
            Protocol.readCallbackHello(ByteArrayInputStream(out.toByteArray())),
        )
        assertFailsWith<ProtocolException> { Protocol.readCallbackHello(ByteArrayInputStream("HOPY\u0000".toByteArray())) }
        // Announces stay unchanged on the wire.
        assertFalse(String(DiscoveryMessage(DiscoveryMessage.ANNOUNCE, me, 1).encode()).contains("token"))
    }

    @Test
    fun callbackTokensLimitsAndAddresses() {
        val t = Protocol.newToken()
        assertTrue(Protocol.isValidToken(t))
        assertFalse(Protocol.isValidToken(t.uppercase().replace('0', 'A')))
        assertTrue(Protocol.tokensEqual(t, String(t.toCharArray())))
        assertFalse(Protocol.tokensEqual(t, Protocol.newToken()))
        val l = CallbackLimiter()
        val a = "192.168.1.2"
        assertTrue(l.admit(a, "t1", 0))
        assertFalse(l.admit(a, "t2", 0), "one outstanding per IP")
        l.release(a)
        assertTrue(l.admit(a, "t1", 0), "failed connect: the token may retry")
        l.markUsed("t1", 0)
        l.release(a)
        assertFalse(l.admit(a, "t1", 0), "token that connected back")
        for (i in 3..DiscoveryNode.MAX_CALLBACKS_PER_MINUTE) { assertTrue(l.admit(a, "t$i", 0)); l.release(a) }
        assertFalse(l.admit(a, "fresh", 0), "per-IP cap")
        repeat(DiscoveryNode.MAX_CALLBACKS_PER_MINUTE_TOTAL - DiscoveryNode.MAX_CALLBACKS_PER_MINUTE) {
            assertTrue(l.admit("10.0.0.$it", "u$it", 0))
        }
        assertFalse(l.admit("10.0.1.1", "other", 0), "global cap")
        assertTrue(l.admit("10.0.1.1", "other", 61_000))
        assertFalse(l.admit("10.0.1.2", "t1", 61_000), "used tokens remembered for 2 min")
        assertTrue(l.admit("10.0.1.2", "t1", 181_000))
        val known = InetAddress.getByName("192.168.1.20")
        assertTrue(DiscoveryNode.trustedSource(known, InetAddress.getByName("192.168.1.20")))
        assertFalse(DiscoveryNode.trustedSource(known, InetAddress.getByName("192.168.1.21")))
        assertFalse(DiscoveryNode.trustedSource(InetAddress.getByName("8.8.8.8"), InetAddress.getByName("8.8.8.8")))
        val big = ByteArrayOutputStream().apply {
            write("HOPC".toByteArray()); write(0); write(byteArrayOf(0, 0, 4, 1))
        }
        assertFailsWith<ProtocolException> { Protocol.readCallbackHello(ByteArrayInputStream(big.toByteArray())) }
        val list = ByteArrayOutputStream().apply {
            write("HOPC".toByteArray()); write(0); write(byteArrayOf(0, 0, 0, 5)); write("[1,2]".toByteArray())
        }
        assertFailsWith<ProtocolException> { Protocol.readCallbackHello(ByteArrayInputStream(list.toByteArray())) }
        for (ok in listOf("127.0.0.1", "10.1.2.3", "172.31.0.1", "192.168.0.9", "100.64.0.1", "169.254.1.1", "::1", "fd00::1", "fe80::1")) {
            assertTrue(DiscoveryNode.isLocalAddress(InetAddress.getByName(ok)), ok)
        }
        for (bad in listOf("8.8.8.8", "172.32.0.1", "100.128.0.1", "2001:db8::1")) {
            assertFalse(DiscoveryNode.isLocalAddress(InetAddress.getByName(bad)), bad)
        }
    }

    /** Spec §6: the receiver's TCP port is unreachable, so it connects back to the sender. */
    @Test
    fun callbackDeliversWhenDirectConnectFails() {
        val src = Files.createTempDirectory("src").toFile()
        val dst = Files.createTempDirectory("dst").toFile()
        val data = ByteArray(700_000) { (it % 247).toByte() }
        File(src, "cb.bin").writeBytes(data)
        val closed = ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { it.localPort }
        val rx = DiscoveryNode(DeviceInfo("rx", "Receiver", "windows"), bindPort = 0, useMulticast = false)
        rx.transferPort = closed // announced, but nothing listens: stands in for a firewall
        rx.start()
        val rec = Recorder(accept = true)
        CallbackReceiver.install(rx, DirectorySink(dst), rec, Local.ephemeral(rx.me))
        val tx = DiscoveryNode(
            me, bindPort = 0, useMulticast = false,
            seeds = listOf(InetSocketAddress(InetAddress.getLoopbackAddress(), rx.localPort)),
        )
        tx.start()
        val deadline = System.currentTimeMillis() + 3_000
        while (tx.peers().isEmpty() && System.currentTimeMillis() < deadline) Thread.sleep(50)
        val peer = tx.peers().single()
        var fallbacks = 0
        var bytesBeforeFallback = false
        var session: SessionInfo? = null
        val outcome = TransferClient.sendToPeer(
            peer, local, listOf(FileSource(File(src, "cb.bin"))), tx,
            object : SendListener {
                override fun onFallback() { fallbacks++ }
                override fun onSession(info: SessionInfo) { session = info }
                override fun onProgress(done: Long, total: Long) { if (fallbacks == 0) bytesBeforeFallback = true }
            },
        )
        assertEquals(SendOutcome.Delivered, outcome)
        assertEquals(1, fallbacks, "fallback signalled once when direct TCP failed")
        assertFalse(bytesBeforeFallback)
        assertEquals(1, peer.crypto, "v1 announce")
        assertTrue(session!!.encrypted, "callback sessions are encrypted too")
        assertEquals(session!!.sas, rec.session?.sas, "same code on both ends")
        val end = System.currentTimeMillis() + 3_000
        while (rec.saved == null && System.currentTimeMillis() < end) Thread.sleep(20)
        assertNull(rec.error)
        assertContentEquals(data, File(dst, "cb.bin").readBytes())
        assertFailsWith<PeerUnreachableException> {
            TransferClient.sendToPeer(peer, local, listOf(FileSource(File(src, "cb.bin"))), node = null)
        }
        tx.stop()
        rx.stop()
    }

    /**
     * Spec §6 sender steps 2–3: while a callback is pending only the receiver's known IP
     * speaks for its id (127.0.0.2 stands in for another LAN host), connections from other
     * IPs are closed unread, and the hello must arrive whole within 3 s and fit in 1 KiB.
     */
    @Test
    fun callbackSenderIgnoresOtherIpsAndSlowHellos() {
        val lo = InetAddress.getLoopbackAddress()
        val fakeRx = java.net.DatagramSocket(0, lo).apply { soTimeout = 3_000 }
        val tx = DiscoveryNode(me, bindPort = 0, useMulticast = false, seeds = listOf(InetSocketAddress(lo, fakeRx.localPort)))
        tx.start()
        val buf = ByteArray(2048)
        fun recv(sock: java.net.DatagramSocket): java.net.DatagramPacket? =
            try { java.net.DatagramPacket(buf, buf.size).also { sock.receive(it) } } catch (e: java.io.IOException) { null }
        fun nextCallback(sock: java.net.DatagramSocket): DiscoveryMessage? {
            while (true) {
                val p = recv(sock) ?: return null
                val m = DiscoveryMessage.decode(p.data, p.length)
                if (m?.type == DiscoveryMessage.CALLBACK) return m
            }
        }
        val txAddr = recv(fakeRx)!!.socketAddress
        val rxDev = DeviceInfo("rx", "R", "windows")
        val closed = ServerSocket(0, 1, lo).use { it.localPort }
        val announce = DiscoveryMessage(DiscoveryMessage.ANNOUNCE, rxDev, closed).encode()
        fakeRx.send(java.net.DatagramPacket(announce, announce.size, txAddr))
        val deadline = System.currentTimeMillis() + 3_000
        while (tx.peers().isEmpty() && System.currentTimeMillis() < deadline) Thread.sleep(50)
        val peer = tx.peers().single()
        val src = Files.createTempDirectory("src").toFile()
        File(src, "x.txt").writeText("x")
        var outcome: Any? = null
        val sender = thread {
            // The fake receiver announces as v0 (no `crypto`): a plaintext session (spec §7.1).
            outcome = runCatching { TransferClient.sendToPeer(peer, local, listOf(FileSource(File(src, "x.txt"))), tx) }
        }
        val cb = nextCallback(fakeRx)!!
        val token = cb.token!!
        val target = InetSocketAddress(lo, cb.port)

        val other = InetAddress.getByName("127.0.0.2")
        val spoof = runCatching { java.net.DatagramSocket(0, other).apply { soTimeout = 300 } }.getOrNull()
        if (spoof != null) {
            repeat(3) {
                spoof.send(java.net.DatagramPacket(announce, announce.size, txAddr))
                assertNull(nextCallback(spoof), "token sent to a spoofer")
            }
            assertEquals(token, nextCallback(fakeRx)?.token, "re-send still reaches the real receiver")
            spoof.close()
            Socket().use { s ->
                s.bind(InetSocketAddress(other, 0))
                s.connect(target)
                runCatching { Protocol.writeCallbackHello(s.getOutputStream(), "rx", token, crypto = false) }
                s.soTimeout = 5_000
                assertEquals(-1, runCatching { s.getInputStream().read() }.getOrDefault(-1), "other IP dropped")
            }
        }

        fun closesWithin(ms: Long, payload: ByteArray, slow: Boolean = false) = Socket(lo, cb.port).use { s ->
            val t0 = System.currentTimeMillis()
            if (slow) {
                // One byte every 400 ms: never a whole hello; cut off at ~3 s.
                for (i in 0 until 40) {
                    try { s.getOutputStream().write(payload[i % payload.size].toInt()) } catch (e: java.io.IOException) { break }
                    Thread.sleep(400)
                }
            } else {
                s.getOutputStream().write(payload)
                s.soTimeout = 5_000
                runCatching { s.getInputStream().read() }
            }
            System.currentTimeMillis() - t0 < ms
        }
        val hdr = "HOPC".toByteArray() + 0
        assertTrue(closesWithin(2_000, hdr + byteArrayOf(0, 0, 4, 1)), "oversized hello")
        assertTrue(closesWithin(2_000, hdr + byteArrayOf(0, 0, 0, 3) + "\"x\"".toByteArray()), "non-object hello")
        assertTrue(closesWithin(6_000, hdr + byteArrayOf(0, 0, 0, 0x58) + "{\"id\"".toByteArray(), slow = true), "slow hello")

        Socket(lo, cb.port).use { s ->
            Protocol.writeCallbackHello(s.getOutputStream(), "rx", token, crypto = false)
            val input = s.getInputStream()
            Protocol.readHello(input)
            assertEquals("x.txt", Offer.from(Protocol.readJson(input)).files.single().name)
            Protocol.writeJson(s.getOutputStream(), mapOf("accept" to false, "reason" to "declined"))
            sender.join(5_000)
        }
        assertEquals(Result.success(SendOutcome.Declined("declined")), outcome)
        tx.stop()
        fakeRx.close()
    }

    private class Recorder(val accept: Boolean, val auto: Boolean = false) : ReceiveHandler {
        @Volatile var saved: List<String>? = null
        @Volatile var error: String? = null
        @Volatile var session: SessionInfo? = null
        @Volatile var asked = 0
        val blocked = java.util.concurrent.CopyOnWriteArrayList<String>()
        val verified = java.util.concurrent.CopyOnWriteArrayList<PeerKey>()
        val warnings = java.util.concurrent.CopyOnWriteArrayList<Warning>()
        override fun decide(offer: Offer, from: InetAddress, session: SessionInfo): Boolean {
            asked++
            this.session = session
            return accept
        }
        override fun autoAccepts() = auto
        override fun onFinished(from: InetAddress, offer: Offer, saved: List<String>, error: String?) {
            this.error = error
            this.saved = saved
        }
        override fun onBlocked(from: InetAddress, offer: Offer, session: SessionInfo, reason: String) { blocked += reason }
        override fun onVerified(from: InetAddress, peer: PeerKey) { verified += peer }
        override fun onWarning(from: InetAddress, warning: Warning) { warnings += warning }
    }

    private val local = Local.ephemeral(me)
    private val rxLocal = Local.ephemeral(DeviceInfo("rx", "Receiver", "windows"))

    private fun serveOnce(dir: File, handler: ReceiveHandler, rx: Local = rxLocal): Pair<Int, Thread> {
        val server = ServerSocket(0, 1, InetAddress.getLoopbackAddress())
        val t = thread { server.use { TransferServer.handle(it.accept(), DirectorySink(dir), handler, rx) } }
        return server.localPort to t
    }

    /** Serves [n] connections, one after another. */
    private fun serveMany(dir: File, handler: ReceiveHandler, rx: Local, n: Int): Pair<Int, Thread> {
        val server = ServerSocket(0, 50, InetAddress.getLoopbackAddress())
        val t = thread { server.use { repeat(n) { _ -> TransferServer.handle(it.accept(), DirectorySink(dir), handler, rx) } } }
        return server.localPort to t
    }

    @Test
    fun sendAndReceiveFiles() {
        val src = Files.createTempDirectory("src").toFile()
        val dst = Files.createTempDirectory("dst").toFile()
        val big = ByteArray(2_500_000) { (it % 251).toByte() }
        File(src, "big.bin").writeBytes(big)
        File(src, "empty.txt").writeBytes(ByteArray(0))
        File(dst, "big.bin").writeText("old")
        val rec = Recorder(accept = true)
        val (port, t) = serveOnce(dst, rec)
        var last = 0L
        var fallbacks = 0
        var session: SessionInfo? = null
        val peer = Peer(DeviceInfo("rx", "Receiver", "windows"), InetAddress.getLoopbackAddress(), port)
        val outcome = TransferClient.sendToPeer(
            peer, local, listOf(FileSource(File(src, "big.bin")), FileSource(File(src, "empty.txt"))),
            node = null,
            listener = object : SendListener {
                override fun onFallback() { fallbacks++ }
                override fun onSession(info: SessionInfo) { session = info }
                override fun onProgress(done: Long, total: Long) { last = done }
            },
        )
        t.join(5_000)
        assertEquals(SendOutcome.Delivered, outcome)
        assertEquals(0, fallbacks, "no fallback signal on a direct send")
        assertEquals(big.size.toLong(), last)
        // Encrypted (unknown crypto: v1 first), same code on both ends, keys pinned (spec §7.5).
        assertTrue(session!!.encrypted)
        assertEquals(TrustState.NEW, session!!.trust)
        assertEquals(session!!.sas, rec.session!!.sas)
        assertEquals(rxLocal.fingerprint, session!!.peer!!.fingerprint)
        assertEquals(local.fingerprint, rec.session!!.peer!!.fingerprint)
        assertEquals(TrustState.KNOWN, local.trust.state("rx", rxLocal.fingerprint), "sender pins on accept")
        assertEquals("127.0.0.1:$port", local.trust.get("rx")!!.addr)
        assertEquals(TrustState.KNOWN, rxLocal.trust.state(me.id, local.fingerprint), "receiver pins on a person's accept")
        assertNull(rec.error)
        assertContentEquals(big, File(dst, "big (1).bin").readBytes())
        assertEquals("old", File(dst, "big.bin").readText())
        assertTrue(File(dst, "empty.txt").exists())
        assertTrue(dst.listFiles()!!.none { it.name.endsWith(".hoppy-part") })
    }

    @Test
    fun declinedOffer() {
        val src = Files.createTempDirectory("src").toFile()
        File(src, "x.txt").writeText("x")
        val dst = Files.createTempDirectory("dst").toFile()
        val (port, t) = serveOnce(dst, Recorder(accept = false))
        val outcome = TransferClient.send(InetAddress.getLoopbackAddress(), port, local, listOf(FileSource(File(src, "x.txt"))))
        t.join(5_000)
        assertEquals(SendOutcome.Declined("declined"), outcome)
        assertNull(local.trust.get("rx"), "a declined first contact pins nothing")
        assertEquals(0, dst.listFiles()!!.size)
    }

    @Test
    fun closedPortIsUnreachable() {
        val src = Files.createTempDirectory("src").toFile()
        File(src, "x.txt").writeText("x")
        val port = ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { it.localPort }
        assertFailsWith<PeerUnreachableException> {
            TransferClient.send(InetAddress.getLoopbackAddress(), port, local, listOf(FileSource(File(src, "x.txt"))))
        }
    }

    @Test
    fun corruptFileIsDroppedAndTraversalSanitized() {
        val dst = Files.createTempDirectory("dst").toFile()
        val rec = Recorder(accept = true)
        val (port, t) = serveOnce(dst, rec)
        Socket(InetAddress.getLoopbackAddress(), port).use { s ->
            val out = s.getOutputStream()
            Protocol.writeHello(out)
            Protocol.writeJson(out, Offer(me, listOf(FileEntry("../../escape.txt", 2), FileEntry("bad.txt", 3))).toMap())
            assertEquals(true, Protocol.readJson(s.getInputStream())["accept"])
            out.write("ok".toByteArray()); out.write(MessageDigest.getInstance("SHA-256").digest("ok".toByteArray()))
            out.write("bad".toByteArray()); out.write(ByteArray(32))
            val result = Protocol.readJson(s.getInputStream())
            assertEquals(false, result["ok"])
        }
        t.join(5_000)
        assertEquals("ok", File(dst, "escape.txt").readText())
        assertFalse(File(dst, "bad.txt").exists())
        assertTrue(rec.error!!.contains("bad.txt"))
    }

    @Test
    fun discoveryFindsPeerViaSeed() {
        val a = DiscoveryNode(DeviceInfo("node-a", "Alpha", "android"), bindPort = 0, useMulticast = false)
        a.transferPort = 1111
        a.start()
        val b = DiscoveryNode(
            DeviceInfo("node-b", "Beta", "android"), bindPort = 0, useMulticast = false,
            seeds = listOf(InetSocketAddress(InetAddress.getLoopbackAddress(), a.localPort)),
        )
        b.transferPort = 2222
        b.start()
        val deadline = System.currentTimeMillis() + 3_000
        while ((a.peers().isEmpty() || b.peers().isEmpty()) && System.currentTimeMillis() < deadline) Thread.sleep(50)
        assertEquals("Alpha", b.peers().single().device.name)
        assertEquals(1111, b.peers().single().port)
        assertEquals(Protocol.CRYPTO_VERSION, b.peers().single().crypto, "v1 nodes announce crypto")
        assertEquals("Beta", a.peers().single().device.name)
        b.stop()
        Thread.sleep(300)
        assertTrue(a.peers().isEmpty())
        a.stop()
    }
}
