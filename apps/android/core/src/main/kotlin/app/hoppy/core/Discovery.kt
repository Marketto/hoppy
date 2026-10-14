package app.hoppy.core

import java.net.DatagramPacket
import java.net.Inet4Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.MulticastSocket
import java.net.NetworkInterface
import java.net.SocketTimeoutException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArraySet
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

data class Peer(val device: DeviceInfo, val address: InetAddress, val port: Int)

/**
 * LAN discovery (spec §2), pure java.net. On Android the caller must hold a
 * WifiManager.MulticastLock while this runs.
 *
 * [transferPort] can change at runtime: 0 = send-only node (queries, never announces).
 * All socket work happens on background threads, so every public method is safe to
 * call from the Android main thread.
 */
class DiscoveryNode(
    private val me: DeviceInfo,
    private val bindPort: Int = Protocol.DISCOVERY_PORT,
    private val seeds: List<InetSocketAddress> = emptyList(),
    private val useMulticast: Boolean = true,
) {
    @Volatile var transferPort: Int = 0
        set(value) {
            field = value
            if (running) send(if (value == 0) DiscoveryMessage.QUERY else DiscoveryMessage.ANNOUNCE)
        }

    private val peers = ConcurrentHashMap<String, Pair<Peer, Long>>()
    private val listeners = CopyOnWriteArraySet<(List<Peer>) -> Unit>()
    @Volatile private var running = false
    @Volatile private var socket: MulticastSocket? = null
    private val threads = ArrayList<Thread>()

    val localPort: Int get() = socket?.localPort ?: 0

    fun addListener(l: (List<Peer>) -> Unit) { listeners += l }
    fun removeListener(l: (List<Peer>) -> Unit) { listeners -= l }

    private var io: ExecutorService? = null

    /** Opens the socket (on a worker thread) and starts announcing. Waits up to 2 s. */
    @Synchronized
    fun start() {
        if (running) return
        running = true
        val executor = Executors.newSingleThreadExecutor { r -> Thread(r, "hoppy-discovery-io").apply { isDaemon = true } }
        io = executor
        val ready = CountDownLatch(1)
        threads += Thread({ openAndReceive(ready) }, "hoppy-discovery-rx").apply { isDaemon = true; start() }
        ready.await(2, TimeUnit.SECONDS)
    }

    private fun openAndReceive(ready: CountDownLatch) {
        val s = try {
            MulticastSocket(null).apply {
                reuseAddress = true
                broadcast = true
                soTimeout = 1000
                bind(InetSocketAddress(bindPort))
                if (useMulticast) {
                    val group = InetSocketAddress(InetAddress.getByName(Protocol.MULTICAST_GROUP), 0)
                    for (nif in ipv4Interfaces()) runCatching { joinGroup(group, nif) }
                }
            }
        } catch (e: Exception) {
            running = false
            ready.countDown()
            return
        }
        socket = s
        ready.countDown()
        synchronized(this) {
            threads += Thread({ sendLoop() }, "hoppy-discovery-tx").apply { isDaemon = true; start() }
        }
        receiveLoop(s)
    }

    @Synchronized
    fun stop() {
        if (!running) return
        running = false
        val s = socket
        val executor = io
        executor?.execute {
            if (s != null) sendNow(s, DiscoveryMessage.BYE)
            s?.close()
        }
        executor?.shutdown()
        threads.forEach { it.interrupt() }
        threads.clear()
        socket = null
        io = null
        peers.clear()
        notifyListeners()
    }

    /** Ask everyone to announce now. */
    fun refresh() = send(DiscoveryMessage.QUERY)

    fun peers(): List<Peer> {
        val now = System.currentTimeMillis()
        peers.entries.removeIf { now - it.value.second > PEER_TTL_MS }
        return peers.values.map { it.first }.sortedBy { it.device.name.lowercase() }
    }

    private fun sendLoop() {
        send(DiscoveryMessage.QUERY)
        while (running) {
            send(if (transferPort == 0) DiscoveryMessage.QUERY else DiscoveryMessage.ANNOUNCE)
            val before = peers.size
            peers()
            if (peers.size != before) notifyListeners()
            try { Thread.sleep(ANNOUNCE_INTERVAL_MS) } catch (e: InterruptedException) { return }
        }
    }

    private fun receiveLoop(s: MulticastSocket) {
        val buf = ByteArray(2048)
        while (running) {
            val packet = DatagramPacket(buf, buf.size)
            try {
                s.receive(packet)
            } catch (e: SocketTimeoutException) {
                continue
            } catch (e: Exception) {
                if (!running) return else continue
            }
            val msg = DiscoveryMessage.decode(packet.data, packet.length) ?: continue
            if (msg.device.id == me.id) continue
            when (msg.type) {
                DiscoveryMessage.BYE -> if (peers.remove(msg.device.id) != null) notifyListeners()
                else -> {
                    if (msg.port != 0) {
                        val isNew = !peers.containsKey(msg.device.id)
                        val peer = Peer(msg.device, packet.address, msg.port)
                        val old = peers.put(msg.device.id, peer to System.currentTimeMillis())
                        if (isNew || old?.first != peer) notifyListeners()
                    }
                    if (msg.type == DiscoveryMessage.QUERY && transferPort != 0) {
                        val reply = DiscoveryMessage(DiscoveryMessage.ANNOUNCE, me, transferPort).encode()
                        val to = packet.socketAddress
                        runCatching { io?.execute { runCatching { s.send(DatagramPacket(reply, reply.size, to)) } } }
                    }
                }
            }
        }
    }

    private fun send(type: String) {
        val s = socket ?: return
        runCatching { io?.execute { sendNow(s, type) } }
    }

    private fun sendNow(s: MulticastSocket, type: String) {
        val data = DiscoveryMessage(type, me, transferPort).encode()
        val targets = ArrayList<InetSocketAddress>(seeds)
        if (useMulticast) {
            targets += InetSocketAddress(InetAddress.getByName(Protocol.MULTICAST_GROUP), Protocol.DISCOVERY_PORT)
            targets += InetSocketAddress(InetAddress.getByName("255.255.255.255"), Protocol.DISCOVERY_PORT)
        }
        for (t in targets) runCatching { s.send(DatagramPacket(data, data.size, t)) }
    }

    private fun notifyListeners() {
        val snapshot = peers()
        listeners.forEach { runCatching { it(snapshot) } }
    }

    companion object {
        const val ANNOUNCE_INTERVAL_MS = 5_000L
        const val PEER_TTL_MS = 15_000L

        fun ipv4Interfaces(): List<NetworkInterface> =
            NetworkInterface.getNetworkInterfaces()?.toList().orEmpty().filter { nif ->
                runCatching { nif.isUp && !nif.isLoopback && nif.supportsMulticast() }.getOrDefault(false) &&
                    nif.inetAddresses.toList().any { it is Inet4Address }
            }
    }
}
