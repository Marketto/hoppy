package app.hoppy.core

import java.io.File
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import kotlin.system.exitProcess

/**
 * Tiny CLI over the Kotlin core, used by tools/interop.sh to test the Android
 * networking code against the Rust CLI and the Python reference.
 *
 *   send HOST PORT FILE...      receive DIR PORT [UDP_PORT]      peers SEED_PORT
 *   send-peer UDP_PORT [--no-direct] FILE...   (discovers 127.0.0.1:UDP_PORT; callback, spec §6)
 */
fun main(args: Array<String>) {
    val me = DeviceInfo("kotlin-interop", "InteropKotlin", "android")
    when (args.firstOrNull()) {
        "send" -> {
            val outcome = TransferClient.send(
                InetAddress.getByName(args[1]), args[2].toInt(), me, args.drop(3).map { FileSource(File(it)) },
            )
            println(outcome)
            exitProcess(if (outcome == SendOutcome.Delivered) 0 else 1)
        }
        "receive" -> {
            val dir = File(args[1])
            val server = ServerSocket(args[2].toInt())
            val node = (args.getOrNull(3)?.let { DiscoveryNode(me, bindPort = it.toInt(), useMulticast = false) } ?: DiscoveryNode(me))
                .apply { transferPort = server.localPort; start() }
            println("receiving on ${server.localPort}")
            val handler = object : ReceiveHandler {
                override fun decide(offer: Offer, from: InetAddress) = true
                override fun onFinished(from: InetAddress, offer: Offer, saved: List<String>, error: String?) =
                    println("finished ${saved.size} error=$error")
            }
            CallbackReceiver.install(node, DirectorySink(dir), handler)
            while (true) TransferServer.handle(server.accept(), DirectorySink(dir), handler)
            @Suppress("UNREACHABLE_CODE") node.stop()
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
            val outcome = if (direct) TransferClient.sendToPeer(peer, me, files, node)
            else TransferClient.sendViaCallback(peer, me, files, node)
            println(outcome)
            node.stop()
            exitProcess(if (outcome == SendOutcome.Delivered) 0 else 1)
        }
        "peers" -> {
            val node = DiscoveryNode(
                me, bindPort = 0,
                seeds = listOf(InetSocketAddress(InetAddress.getLoopbackAddress(), args[1].toInt())),
            )
            node.start()
            Thread.sleep(2_000)
            node.peers().forEach { println("${it.device.name} ${it.device.platform} ${it.address.hostAddress}:${it.port}") }
            node.stop()
        }
        else -> {
            System.err.println("usage: send HOST PORT FILE... | receive DIR PORT [UDP_PORT] | send-peer UDP_PORT [--no-direct] FILE... | peers SEED_PORT")
            exitProcess(2)
        }
    }
}
