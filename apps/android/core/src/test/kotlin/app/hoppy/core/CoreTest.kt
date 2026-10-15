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

    private class Recorder(val accept: Boolean) : ReceiveHandler {
        @Volatile var saved: List<String>? = null
        @Volatile var error: String? = null
        override fun decide(offer: Offer, from: InetAddress) = accept
        override fun onFinished(from: InetAddress, offer: Offer, saved: List<String>, error: String?) {
            this.error = error
            this.saved = saved
        }
    }

    private fun serveOnce(dir: File, handler: ReceiveHandler): Pair<Int, Thread> {
        val server = ServerSocket(0, 1, InetAddress.getLoopbackAddress())
        val t = thread { server.use { TransferServer.handle(it.accept(), DirectorySink(dir), handler) } }
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
        val outcome = TransferClient.send(
            InetAddress.getLoopbackAddress(), port, me,
            listOf(FileSource(File(src, "big.bin")), FileSource(File(src, "empty.txt"))),
        ) { done, _ -> last = done }
        t.join(5_000)
        assertEquals(SendOutcome.Delivered, outcome)
        assertEquals(big.size.toLong(), last)
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
        val outcome = TransferClient.send(InetAddress.getLoopbackAddress(), port, me, listOf(FileSource(File(src, "x.txt"))))
        t.join(5_000)
        assertEquals(SendOutcome.Declined("declined"), outcome)
        assertEquals(0, dst.listFiles()!!.size)
    }

    @Test
    fun closedPortIsUnreachable() {
        val src = Files.createTempDirectory("src").toFile()
        File(src, "x.txt").writeText("x")
        val port = ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { it.localPort }
        assertFailsWith<PeerUnreachableException> {
            TransferClient.send(InetAddress.getLoopbackAddress(), port, me, listOf(FileSource(File(src, "x.txt"))))
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
        assertEquals("Beta", a.peers().single().device.name)
        b.stop()
        Thread.sleep(300)
        assertTrue(a.peers().isEmpty())
        a.stop()
    }
}
