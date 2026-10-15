package app.hoppy.core

import java.net.DatagramPacket
import java.net.Inet4Address
import java.net.Inet6Address
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
import java.util.concurrent.atomic.AtomicLong

data class Peer(val device: DeviceInfo, val address: InetAddress, val port: Int)

/**
 * A validated, rate-limited request from a sender to connect back to it (spec §6).
 * [address] is the UDP source of the datagram, never an address from its JSON.
 * Call [markConnected] once the TCP connect succeeded and the hello was written: only then
 * is the token recorded as used, so a re-send after a failed connect can retry.
 */
class CallbackRequest(
    val sender: DeviceInfo,
    val address: InetAddress,
    val port: Int,
    val token: String,
    private val onConnected: () -> Unit = {},
) {
    fun markConnected() = onConnected()
}

/** Sender-side outstanding callback request. Close it to stop the re-sends. */
interface CallbackTicket : AutoCloseable {
    /** Sends the callback datagram again to the peer's last known discovery address. */
    fun resend()
    override fun close()
}

/**
 * LAN discovery (spec §2), pure java.net. On Android the caller must hold a
 * WifiManager.MulticastLock while this runs.
 *
 * [transferPort] can change at runtime: 0 = send-only node (queries, never announces).
 * All socket work happens on background threads, so every public method is safe to
 * call from the Android main thread.
 */
class DiscoveryNode(
    val me: DeviceInfo,
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
    /** UDP source of each peer's last datagram: where callback datagrams go. */
    private val udpSources = ConcurrentHashMap<String, InetSocketAddress>()
    /** [peerIp]: where the peer was discovered; only it may speak for [peerId] (spec §6). */
    private class Pending(val peerId: String, val peerIp: InetAddress, val data: ByteArray)
    private val pending = ConcurrentHashMap<Long, Pending>()
    private val nextTicket = AtomicLong(1)
    private val limiter = CallbackLimiter()

    /**
     * Receiver side (spec §6): called on a fresh background thread for each valid callback
     * datagram addressed to this node while [transferPort] != 0. Usually set through
     * [CallbackReceiver.install]. May block for the whole transfer.
     */
    @Volatile var callbackHandler: ((CallbackRequest) -> Unit)? = null
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

    /**
     * Sender side (spec §6): asks [peer] to connect back to our TCP [port] with [token].
     * Sends one datagram now and one each time [peer] announces or queries until closed.
     */
    fun requestCallback(peer: Peer, port: Int, token: String): CallbackTicket {
        val key = nextTicket.getAndIncrement()
        val data = DiscoveryMessage(DiscoveryMessage.CALLBACK, me, port, token, peer.device.id).encode()
        pending[key] = Pending(peer.device.id, peer.address, data)
        val fallback = InetSocketAddress(peer.address, Protocol.DISCOVERY_PORT)
        val ticket = object : CallbackTicket {
            override fun resend() {
                val s = socket ?: return
                val target = udpSources[peer.device.id]?.takeIf { it.address == peer.address } ?: fallback
                runCatching { io?.execute { runCatching { s.send(DatagramPacket(data, data.size, target)) } } }
            }
            override fun close() { pending.remove(key) }
        }
        ticket.resend()
        return ticket
    }

    fun peers(): List<Peer> {
        val now = System.currentTimeMillis()
        peers.entries.removeIf { now - it.value.second > PEER_TTL_MS }
        udpSources.keys.retainAll(peers.keys)
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
                DiscoveryMessage.BYE -> {
                    udpSources.remove(msg.device.id)
                    if (peers.remove(msg.device.id) != null) notifyListeners()
                }
                DiscoveryMessage.CALLBACK -> handleCallback(msg, packet.address)
                else -> {
                    val source = packet.socketAddress as InetSocketAddress
                    // While we wait for this id to connect back, only its known IP may speak
                    // for it: a claim from elsewhere would collect the token (spec §6 step 2).
                    if (pending.values.any { it.peerId == msg.device.id && !trustedSource(it.peerIp, packet.address) }) continue
                    if (msg.port != 0) udpSources[msg.device.id] = source
                    // A firewalled receiver accepts unicast replies to its announces for a few
                    // seconds only: answer pending callback requests right now (spec §6).
                    for (p in pending.values) {
                        if (p.peerId == msg.device.id) {
                            runCatching { io?.execute { runCatching { s.send(DatagramPacket(p.data, p.data.size, source)) } } }
                        }
                    }
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

    private fun handleCallback(msg: DiscoveryMessage, from: InetAddress) {
        val handler = callbackHandler ?: return
        if (transferPort == 0 || msg.to != me.id || !isLocalAddress(from)) return
        val token = msg.token ?: return
        val ip = from.hostAddress ?: return
        if (!limiter.admit(ip, token, System.currentTimeMillis())) return
        val request = CallbackRequest(msg.device, from, msg.port, token) {
            limiter.markUsed(token, System.currentTimeMillis())
        }
        try {
            Thread({
                try { handler(request) } catch (e: Exception) { /* handler reports its own errors */ } finally { limiter.release(ip) }
            }, "hoppy-callback").apply { isDaemon = true; start() }
        } catch (e: Throwable) {
            limiter.release(ip)
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
        /** Spec §6 timings; same values in Rust and Python. */
        const val CALLBACK_WINDOW_MS = 20_000L
        const val CALLBACK_RESEND_MS = 2_000L
        const val CALLBACK_CONNECT_TIMEOUT_MS = 3_000
        /** Receiver-side cap on callback attempts per rolling minute, per source IP. */
        const val MAX_CALLBACKS_PER_MINUTE = 10
        /** Receiver-side cap on callback attempts per rolling minute, all sources together. */
        const val MAX_CALLBACKS_PER_MINUTE_TOTAL = 30

        /** [from] may speak for a peer discovered at [known] (spec §6 sender step 2). */
        fun trustedSource(known: InetAddress, from: InetAddress): Boolean = known == from && isLocalAddress(from)

        /** Loopback, RFC 1918, 100.64/10, link-local, IPv6 ULA (fc00::/7) / link-local. */
        fun isLocalAddress(a: InetAddress): Boolean {
            val b = a.address
            if (a.isLoopbackAddress || a.isLinkLocalAddress) return true
            if (a is Inet6Address) return (b[0].toInt() and 0xfe) == 0xfc // IPv4-mapped become Inet4Address
            val x = b[0].toInt() and 0xff
            val y = b[1].toInt() and 0xff
            return a.isSiteLocalAddress || (x == 100 && y in 64..127)
        }

        fun ipv4Interfaces(): List<NetworkInterface> =
            NetworkInterface.getNetworkInterfaces()?.toList().orEmpty().filter { nif ->
                runCatching { nif.isUp && !nif.isLoopback && nif.supportsMulticast() }.getOrDefault(false) &&
                    nif.inetAddresses.toList().any { it is Inet4Address }
            }
    }
}

/**
 * Receiver-side callback limits (spec §6): 1 outstanding per IP, 10/min per IP, 30/min in
 * total; a token is remembered for 2 min only once it connected back ([markUsed]).
 */
class CallbackLimiter {
    private val used = LinkedHashMap<String, Long>()
    private val outstanding = HashSet<String>()
    private val started = ArrayDeque<Pair<String, Long>>()

    /** If true, call [release] when the attempt ends and [markUsed] if it connected. */
    @Synchronized
    fun admit(ip: String, token: String, nowMs: Long): Boolean {
        used.entries.removeIf { nowMs - it.value > TOKEN_MEMORY_MS }
        while (started.isNotEmpty() && nowMs - started.first().second >= 60_000L) started.removeFirst()
        val fromIp = started.count { it.first == ip }
        if (token in used || ip in outstanding || fromIp >= DiscoveryNode.MAX_CALLBACKS_PER_MINUTE ||
            started.size >= DiscoveryNode.MAX_CALLBACKS_PER_MINUTE_TOTAL
        ) return false
        outstanding += ip
        started.addLast(ip to nowMs)
        return true
    }

    @Synchronized
    fun markUsed(token: String, nowMs: Long) {
        if (used.size >= MAX_TOKENS) used.remove(used.keys.first())
        used[token] = nowMs
    }

    @Synchronized
    fun release(ip: String) { outstanding -= ip }

    private companion object {
        const val TOKEN_MEMORY_MS = 120_000L
        const val MAX_TOKENS = 256
    }
}
