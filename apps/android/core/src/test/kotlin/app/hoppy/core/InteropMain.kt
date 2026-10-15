package app.hoppy.core

import java.io.File
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import kotlin.system.exitProcess

/**
 * Tiny CLI over the Kotlin core, used by tools/interop.sh to test the Android networking
 * and v1 encryption code against the Rust CLI and the Python reference.
 *
 *   send HOST PORT FILE...                    receive DIR PORT [UDP_PORT]
 *   send-peer UDP_PORT [--no-direct] FILE...  (discovers 127.0.0.1:UDP_PORT; callback, spec §6)
 *   peers SEED_PORT
 *
 * Identity: with HOPPY_KT_DIR set, the key and trust store live there (identity.key,
 * trust.json) and are shared by every process, like Android's filesDir; else in memory.
 * Output: sessions print "🔒 encrypted · <trust> · <4 emoji>" or "NOT encrypted".
 */
fun main(args: Array<String>) {
    val me = DeviceInfo("kotlin-interop", "InteropKotlin", "android")
    val local = System.getenv("HOPPY_KT_DIR")?.let { dir ->
        Local(me, DeviceKey.loadOrCreate(File(dir, "identity.key")), Trust.open(File(dir, "trust.json")))
    } ?: Local.ephemeral(me)
    fun describe(s: SessionInfo) =
        if (!s.encrypted) "NOT encrypted" else "🔒 encrypted · ${s.trust?.wire} · ${s.sas?.text}"
    val printer = object : SendListener {
        override fun onSession(info: SessionInfo) = println("session ${describe(info)}")
        override fun onWarning(warning: Warning) = println("warning $warning")
    }
    fun finish(outcome: Result<SendOutcome>): Nothing {
        outcome.onSuccess { println(it) }.onFailure { println("error ${it::class.simpleName}: ${it.message}") }
        exitProcess(if (outcome.getOrNull() == SendOutcome.Delivered) 0 else 1)
    }
    when (args.firstOrNull()) {
        "send" -> finish(
            runCatching {
                TransferClient.send(
                    InetAddress.getByName(args[1]), args[2].toInt(), local, args.drop(3).map { FileSource(File(it)) }, printer,
                )
            },
        )
        "receive" -> {
            val rest = args.drop(1)
            val dir = File(rest[0])
            val server = ServerSocket(rest[1].toInt())
            val node = (rest.getOrNull(2)?.let { DiscoveryNode(me, bindPort = it.toInt(), useMulticast = false) } ?: DiscoveryNode(me))
                .apply { transferPort = server.localPort; start() }
            println("receiving on ${server.localPort} fingerprint ${local.fingerprint}")
            System.out.flush()
            val handler = object : ReceiveHandler {
                override fun decide(offer: Offer, from: InetAddress, session: SessionInfo): Boolean {
                    println("offer from ${offer.sender.name}: ${describe(session)}")
                    return true
                }
                override fun autoAccepts() = true // like `hoppy receive --yes`: never pins
                override fun onFinished(from: InetAddress, offer: Offer, saved: List<String>, error: String?) =
                    println("finished ${saved.size} error=$error")
                override fun onBlocked(from: InetAddress, offer: Offer, session: SessionInfo, reason: String) =
                    println("blocked ${offer.sender.name}: $reason")
                override fun onWarning(from: InetAddress, warning: Warning) = when (warning) {
                    is Warning.HandshakeAborts ->
                        println("WARNING: ${warning.recent} encrypted connections broken off right after the handshake")
                    is Warning.TrustNotSaved -> println("WARNING: ${warning.message}")
                }
            }
            CallbackReceiver.install(node, DirectorySink(dir), handler, local)
            while (true) {
                val client = server.accept()
                Thread { TransferServer.handle(client, DirectorySink(dir), handler, local) }.start()
            }
        }
        "send-peer" -> {
            val direct = args[2] != "--no-direct"
            val files = args.drop(if (direct) 2 else 3).map { FileSource(File(it)) }
            val node = DiscoveryNode(
                me, bindPort = 0, useMulticast = false,
                seeds = listOf(InetSocketAddress(InetAddress.getLoopbackAddress(), args[1].toInt())),
            )
            node.start()
            val deadline = System.currentTimeMillis() + 5_000
            while (node.peers().isEmpty() && System.currentTimeMillis() < deadline) Thread.sleep(50)
            val peer = node.peers().firstOrNull() ?: run { System.err.println("no peer"); exitProcess(1) }
            val outcome = runCatching {
                if (direct) TransferClient.sendToPeer(peer, local, files, node, printer)
                else TransferClient.sendViaCallback(peer, local, files, node, printer)
            }
            node.stop()
            finish(outcome)
        }
        "peers" -> {
            val node = DiscoveryNode(
                me, bindPort = 0,
                seeds = listOf(InetSocketAddress(InetAddress.getLoopbackAddress(), args[1].toInt())),
            )
            node.start()
            Thread.sleep(2_000)
            node.peers().forEach {
                println("${it.device.name} ${it.device.platform} ${if (it.encrypts) "v1" else "v0"} ${it.address.hostAddress}:${it.port}")
            }
            node.stop()
        }
        else -> {
            System.err.println(
                "usage: send HOST PORT FILE... | receive DIR PORT [UDP_PORT] | " +
                    "send-peer UDP_PORT [--no-direct] FILE... | peers SEED_PORT",
            )
            exitProcess(2)
        }
    }
}
