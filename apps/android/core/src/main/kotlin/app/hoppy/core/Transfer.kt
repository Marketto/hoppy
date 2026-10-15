package app.hoppy.core

import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.EOFException
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.Inet6Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketException
import java.net.SocketTimeoutException
import java.security.MessageDigest

/** Something to send: Android wraps content URIs, tests wrap files. */
interface Source {
    val name: String
    val size: Long
    fun open(): InputStream
}

class FileSource(private val file: File) : Source {
    override val name: String get() = file.name
    override val size: Long get() = file.length()
    override fun open(): InputStream = file.inputStream()
}

sealed interface SendOutcome {
    data object Delivered : SendOutcome
    data class Declined(val reason: String?) : SendOutcome
}

/**
 * The TCP connection to the receiver could not be opened (timeout, refused, no route):
 * typically a firewall, a different network or client isolation. Nothing was sent.
 */
class PeerUnreachableException(cause: IOException) : IOException(cause.message, cause)

/**
 * The receiver answered our encrypted hello with the empty "busy" frame (spec §7.3: it is
 * limiting aborted handshakes). Try again in a minute; never a reason to send in plaintext.
 */
class ReceiverBusyException : IOException("busy: the receiver is limiting connections, try again in a minute")

/**
 * A device that must encrypt (it advertised `crypto`, or its key / address is remembered)
 * did not answer the encrypted handshake: possible downgrade attack. Nothing was sent.
 */
class UnencryptedException(message: String) : IOException(message)

/** The other side answered with an error (Result / verification reply). */
class RemoteException(message: String) : IOException(message)

/** Non-fatal security problems for the UI (spec §7.3, §7.5). */
sealed interface Warning {
    /**
     * Encrypted connections from a source (or from many) were abandoned right after this
     * device's half of the handshake, [recent] times in the last minute: someone may be
     * trying to guess the verification code. Further attempts are slowed down.
     */
    data class HandshakeAborts(val recent: Int) : Warning
    /** The list of known devices could not be saved (the device will look new next time). */
    data class TrustNotSaved(val message: String) : Warning
}

/** Sender-side events, all on the sending thread. */
interface SendListener {
    /** The direct connection failed; asking the peer to connect back (spec §6, up to 20 s). */
    fun onFallback() {}
    /** Before the offer goes out: encrypted or not, trust state, SAS to show while waiting. */
    fun onSession(info: SessionInfo) {}
    fun onProgress(done: Long, total: Long) {}
    fun onWarning(warning: Warning) {}

    companion object {
        val NONE = object : SendListener {}
    }
}

/**
 * This device as a party to sessions (spec §7.5): who it says it is, its static key, the
 * trust store, the plaintext policy and the aborted-handshake guard (share one per process).
 */
class Local(
    val device: DeviceInfo,
    val key: DeviceKey,
    val trust: Trust,
    val allowPlaintext: Boolean = Protocol.ALLOW_PLAINTEXT_DEFAULT,
    val guard: HandshakeGuard = HandshakeGuard(),
    /** [TransferClient.SESSION_DEADLINE_MS]; shorter only in tests. */
    val sessionDeadlineMs: Long = TransferClient.SESSION_DEADLINE_MS,
) {
    val fingerprint: String get() = key.fingerprint

    fun withDevice(d: DeviceInfo) = Local(d, key, trust, allowPlaintext, guard, sessionDeadlineMs)

    companion object {
        /** A throw-away identity: fresh key, empty in-memory trust store (tests, tools). */
        fun ephemeral(device: DeviceInfo) = Local(device, DeviceKey.generate(), Trust.inMemory())
    }
}

/** `ip:port` as Rust prints a `SocketAddr` (IPv6 in brackets, no scope): the trust store's `addr`. */
fun addrKey(address: InetAddress, port: Int): String {
    val host = address.hostAddress?.substringBefore('%') ?: "?"
    return if (address is Inet6Address) "[$host]:$port" else "$host:$port"
}

/**
 * Per-read timeout plus an optional overall deadline (monotonic ms), changeable while the
 * session runs: trickled bytes cannot extend the deadline.
 */
internal class Timing(@Volatile var perReadMs: Int, @Volatile var deadlineMs: Long?) {
    fun set(perRead: Int, deadline: Long?) {
        perReadMs = perRead
        deadlineMs = deadline
    }
}

internal class TimedInputStream(private val socket: Socket, private val timing: Timing) : InputStream() {
    private val input = socket.getInputStream()

    private fun arm() {
        val deadline = timing.deadlineMs
        var wait = timing.perReadMs.toLong()
        if (deadline != null) {
            val left = deadline - HandshakeGuard.nowMs()
            if (left <= 0) throw SocketTimeoutException("no answer in time")
            wait = minOf(wait, left)
        }
        socket.soTimeout = wait.coerceIn(1, Int.MAX_VALUE.toLong()).toInt()
    }

    override fun read(): Int { arm(); return input.read() }
    override fun read(b: ByteArray, off: Int, len: Int): Int { arm(); return input.read(b, off, len) }
    override fun close() = input.close()
}

/** The peer closed or reset right after our v1 hello: a v0 node. */
private class PeerClosedException : IOException("closed during the encrypted handshake")

private fun closedByPeer(e: IOException) = e is EOFException || e is SocketException

object TransferClient {
    private const val CONNECT_TIMEOUT_MS = 5_000
    /** Direct attempt before falling back to a callback (spec §6). */
    const val DIRECT_CONNECT_TIMEOUT_MS = 3_000
    /** One deadline for the whole callback hello, not per read (spec §6 step 3). */
    private const val CALLBACK_HELLO_TIMEOUT_MS = 3_000L
    private const val DECISION_TIMEOUT_MS = 120_000 // a human may be deciding
    internal const val IO_TIMEOUT_MS = 30_000
    /** Sender: one deadline for handshake + Decision (spec §7.2). */
    const val SESSION_DEADLINE_MS = 130_000L

    /**
     * Sends to a typed address (no discovery: no id, no callback). Encrypted unless the
     * receiver closes right after the v1 hello (an old app) and plaintext is allowed.
     */
    fun send(
        host: InetAddress,
        port: Int,
        local: Local,
        sources: List<Source>,
        listener: SendListener = SendListener.NONE,
    ): SendOutcome {
        require(sources.isNotEmpty()) { "nothing to send" }
        val peer = Peer.at(host, port)
        val socket = connect(peer, CONNECT_TIMEOUT_MS)
        return sendSession(socket, peer.crypto, false, peer, local, sources, listener)
    }

    /**
     * Sends to a discovered [peer]: direct TCP first (3 s); if that cannot connect and [node]
     * is given, asks the peer to connect back (spec §6). Blocks up to ~23 s before
     * [PeerUnreachableException]; [KeyChangedException] if the peer's key is not the
     * remembered one (nothing sent). Not on the main thread.
     */
    fun sendToPeer(
        peer: Peer,
        local: Local,
        sources: List<Source>,
        node: DiscoveryNode?,
        listener: SendListener = SendListener.NONE,
    ): SendOutcome {
        require(sources.isNotEmpty()) { "nothing to send" }
        val direct = try {
            connect(peer, DIRECT_CONNECT_TIMEOUT_MS)
        } catch (e: PeerUnreachableException) {
            if (node == null || peer.device.id.isEmpty()) throw e
            return sendViaCallback(peer, local, sources, node, listener)
        }
        return sendSession(direct, peer.crypto, false, peer, local, sources, listener)
    }

    /** Callback only (spec §6); [sendToPeer] calls this when the direct connect fails. */
    fun sendViaCallback(
        peer: Peer,
        local: Local,
        sources: List<Source>,
        node: DiscoveryNode,
        listener: SendListener = SendListener.NONE,
    ): SendOutcome {
        require(sources.isNotEmpty()) { "nothing to send" }
        listener.onFallback()
        val (socket, hello) = connectViaCallback(peer, node)
        return sendSession(socket, maxCrypto(hello.crypto, peer.crypto), true, peer, local, sources, listener)
    }

    private fun maxCrypto(a: Int?, b: Int?): Int? = if (a == null) b else if (b == null) a else maxOf(a, b)

    private fun connect(peer: Peer, timeoutMs: Int): Socket {
        val s = Socket()
        try {
            s.connect(InetSocketAddress(peer.address, peer.port), timeoutMs)
        } catch (e: IOException) {
            s.close()
            throw PeerUnreachableException(e)
        }
        return s
    }

    private fun connectViaCallback(peer: Peer, node: DiscoveryNode): Pair<Socket, CallbackHello> {
        ServerSocket(0).use { server ->
            server.soTimeout = 250
            val token = Protocol.newToken()
            node.requestCallback(peer, server.localPort, token).use { ticket ->
                val deadline = System.currentTimeMillis() + DiscoveryNode.CALLBACK_WINDOW_MS
                var nextResend = System.currentTimeMillis() + DiscoveryNode.CALLBACK_RESEND_MS
                while (System.currentTimeMillis() < deadline) {
                    val client = try {
                        server.accept()
                    } catch (e: SocketTimeoutException) {
                        null
                    }
                    // Only the peer's known IP may connect back (spec §6 step 3): anything else
                    // is closed unread. A wrong, slow or silent client is dropped too.
                    if (client != null && client.inetAddress != peer.address) {
                        client.close()
                    } else if (client != null) {
                        try {
                            val input = DeadlineInputStream(client, System.currentTimeMillis() + CALLBACK_HELLO_TIMEOUT_MS)
                            val hello = Protocol.readCallbackHello(input) // unbuffered: no read-ahead
                            if (Protocol.tokensEqual(hello.token, token) && hello.id == peer.device.id) return client to hello
                        } catch (e: IOException) {
                            // fall through
                        }
                        client.close()
                    }
                    if (System.currentTimeMillis() >= nextResend) {
                        ticket.resend()
                        nextResend = System.currentTimeMillis() + DiscoveryNode.CALLBACK_RESEND_MS
                    }
                }
            }
        }
        throw PeerUnreachableException(
            IOException("${peer.device.name} did not connect directly or back within ${DiscoveryNode.CALLBACK_WINDOW_MS / 1000} s"),
        )
    }

    /** Which session the sender uses (spec §7.1). */
    internal enum class Plan { PLAINTEXT, ENCRYPTED, ENCRYPTED_OR_FALLBACK }

    /** [crypto]: what the peer advertised (null = unknown); [pinned]: we hold its key or it encrypted at this address. */
    internal fun plan(crypto: Int?, pinned: Boolean, allowPlaintext: Boolean): Plan = when {
        crypto != null && crypto >= Protocol.CRYPTO_VERSION -> Plan.ENCRYPTED
        pinned || !allowPlaintext -> Plan.ENCRYPTED
        crypto != null -> Plan.PLAINTEXT
        else -> Plan.ENCRYPTED_OR_FALLBACK
    }

    internal class Opened(val tx: CipherState, val rx: CipherState, val info: SessionInfo)

    /**
     * Initiator side of spec §7.3 on a fresh connection: `HOPY 0x01` + message 1, read
     * message 2, check the responder (expected id, then the trust store: a changed key aborts
     * before we reveal anything), write message 3.
     */
    internal fun openInitiator(socket: Socket, input: InputStream, local: Local, expectedId: String?): Opened {
        socket.tcpNoDelay = true
        val out = socket.getOutputStream()
        val init = Initiator(local.key)
        val first = java.io.ByteArrayOutputStream()
        first.write(Noise.PROLOGUE)
        Noise.writeFrame(first, init.writeMsg1())
        try {
            out.write(first.toByteArray())
            out.flush()
        } catch (e: IOException) {
            if (closedByPeer(e)) throw PeerClosedException()
            throw e
        }
        val msg2 = try {
            Noise.readFrame(input, Noise.MAX_HANDSHAKE_MESSAGE)
        } catch (e: IOException) {
            if (closedByPeer(e)) throw PeerClosedException()
            throw e
        } ?: throw PeerClosedException()
        // An empty frame: a v1 receiver refusing for now (aborted-handshake limit, §7.3).
        if (msg2.isEmpty()) throw ReceiverBusyException()
        val remote = init.readMsg2(msg2)
        val peer = PeerKey.of(remote)
        if (!expectedId.isNullOrEmpty() && peer.id != expectedId) {
            throw CryptoException("the device at this address is not the one chosen (${remote.device.name})")
        }
        val state = local.trust.state(peer.id, peer.fingerprint)
        if (state == TrustState.KEY_CHANGED) throw KeyChangedException(peer)
        Noise.writeFrame(out, init.writeMsg3(local.device))
        out.flush()
        val (tx, rx) = init.finish()
        val nameMatches = state == TrustState.NEW && local.trust.nameTakenByOther(peer.id, peer.name)
        return Opened(tx, rx, SessionInfo(true, peer, state, remote.sas, nameMatches))
    }

    /** Picks v1 or v0 for this connection (spec §7.1) and runs the sender side. Closes [socket]. */
    private fun sendSession(
        socket: Socket,
        advertised: Int?,
        viaCallback: Boolean,
        peer: Peer,
        local: Local,
        sources: List<Source>,
        listener: SendListener,
    ): SendOutcome {
        val offer = Offer(local.device, sources.map { FileEntry(it.name, it.size) })
        val addr = addrKey(peer.address, peer.port)
        // A known id, or an address where a remembered device completed an encrypted
        // transfer: that device speaks v1, plaintext would be a downgrade (spec §7.1).
        val pinned = (peer.device.id.isNotEmpty() && local.trust.get(peer.device.id) != null) || local.trust.encryptedAt(addr)
        fun newTiming() = Timing(IO_TIMEOUT_MS, HandshakeGuard.nowMs() + local.sessionDeadlineMs)
        val plan = plan(advertised, pinned, local.allowPlaintext)
        socket.use { s ->
            if (plan == Plan.PLAINTEXT) {
                listener.onSession(SessionInfo.PLAINTEXT)
                return sendOver(s, newTiming(), null, addr, local, offer, sources, listener)
            }
            val timing = newTiming()
            val input = BufferedInputStream(TimedInputStream(s, timing), Protocol.CHUNK)
            val opened = try {
                openInitiator(s, input, local, peer.device.id)
            } catch (e: PeerClosedException) {
                if (plan == Plan.ENCRYPTED_OR_FALLBACK && !viaCallback) {
                    s.close()
                    connect(peer, CONNECT_TIMEOUT_MS).use { again ->
                        listener.onSession(SessionInfo.PLAINTEXT)
                        return sendOver(again, newTiming(), null, addr, local, offer, sources, listener)
                    }
                }
                if ((advertised ?: 0) >= Protocol.CRYPTO_VERSION) {
                    throw ProtocolException("connection closed during the encrypted handshake")
                }
                throw UnencryptedException(
                    "${peer.device.name} cannot encrypt" +
                        if (pinned) " although it did before: possible downgrade attack" else " (old Hoppy version?)",
                )
            }
            listener.onSession(opened.info)
            return sendOver(s, timing, Secure(input, opened), addr, local, offer, sources, listener)
        }
    }

    private class Secure(val input: InputStream, val opened: Opened)

    /**
     * Spec §3 from the sender's side over an open connection, in plaintext ([secure] null:
     * writes the v0 hello first) or inside the v1 session. [timing] carries the session
     * deadline (handshake + Decision).
     */
    private fun sendOver(
        socket: Socket,
        timing: Timing,
        secure: Secure?,
        addr: String,
        local: Local,
        offer: Offer,
        sources: List<Source>,
        listener: SendListener,
    ): SendOutcome {
        val total = offer.totalSize
        socket.tcpNoDelay = true
        val input: InputStream
        val out: OutputStream
        if (secure == null) {
            input = BufferedInputStream(TimedInputStream(socket, timing))
            out = BufferedOutputStream(socket.getOutputStream(), Protocol.CHUNK)
            Protocol.writeHello(out)
        } else {
            input = NoiseInputStream(secure.input, secure.opened.rx)
            out = NoiseOutputStream(socket.getOutputStream(), secure.opened.tx)
        }
        Protocol.writeJson(out, offer.toMap())
        out.flush()

        timing.perReadMs = DECISION_TIMEOUT_MS
        val decision = Protocol.readJson(input)
        timing.set(IO_TIMEOUT_MS, null)
        if (decision["accept"] != true) return SendOutcome.Declined(decision["reason"] as? String)
        // TOFU: remembered once a transfer was accepted, with the address it was reached at.
        val peerKey = secure?.opened?.info?.peer
        if (peerKey != null) {
            try {
                local.trust.pin(peerKey, addr)
            } catch (e: IOException) {
                listener.onWarning(Warning.TrustNotSaved(e.message ?: "I/O error"))
            }
        }

        val buf = ByteArray(Protocol.CHUNK)
        var done = 0L
        for (source in sources) {
            val digest = MessageDigest.getInstance("SHA-256")
            var remaining = source.size
            source.open().use { stream ->
                while (remaining > 0) {
                    val n = stream.read(buf, 0, minOf(remaining, buf.size.toLong()).toInt())
                    if (n < 0) throw IOException("${source.name} shrank while sending")
                    digest.update(buf, 0, n)
                    out.write(buf, 0, n)
                    remaining -= n
                    done += n
                    listener.onProgress(done, total)
                }
            }
            out.write(digest.digest())
        }
        out.flush()

        val result = Protocol.readJson(input)
        if (result["ok"] == true) return SendOutcome.Delivered
        throw RemoteException("receiver error: ${result["error"] as? String ?: "unknown"}")
    }
}

/**
 * Reads [socket] until one overall [deadlineMs], so a peer trickling one byte at a time
 * cannot hold the reader longer than that (slow-loris).
 */
private class DeadlineInputStream(private val socket: Socket, private val deadlineMs: Long) : InputStream() {
    private val input = socket.getInputStream()

    private fun arm() {
        val left = deadlineMs - System.currentTimeMillis()
        if (left <= 0) throw SocketTimeoutException("callback hello deadline passed")
        socket.soTimeout = left.coerceAtMost(Int.MAX_VALUE.toLong()).toInt()
    }

    override fun read(): Int { arm(); return input.read() }
    override fun read(b: ByteArray, off: Int, len: Int): Int { arm(); return input.read(b, off, len) }
}

/** Receiver side of spec §6: connect back to a sender that asked for it. */
object CallbackReceiver {
    /**
     * Makes [node] answer callback requests: each runs on its own background thread,
     * connects back, then behaves exactly like [TransferServer.handle] (the handler still
     * decides). Call once after creating the node; set `node.callbackHandler = null` to stop.
     * Apps that must cancel running transfers use [answer] with [onSocket] instead.
     */
    fun install(node: DiscoveryNode, sink: FileSink, handler: ReceiveHandler, local: Local) {
        node.callbackHandler = { request -> answer(request, local, sink, handler) }
    }

    /**
     * Connects to the requester, sends the callback hello (`"crypto":1`), then serves the
     * session. Never throws. [onSocket] gets the socket before connecting, so the caller can
     * close it to cancel (e.g. when the Android service is destroyed).
     */
    fun answer(
        request: CallbackRequest,
        local: Local,
        sink: FileSink,
        handler: ReceiveHandler,
        onSocket: (Socket) -> Unit = {},
    ) {
        val socket = Socket()
        try {
            onSocket(socket)
            socket.connect(InetSocketAddress(request.address, request.port), DiscoveryNode.CALLBACK_CONNECT_TIMEOUT_MS)
            val out = socket.getOutputStream()
            Protocol.writeCallbackHello(out, local.device.id, request.token, crypto = true)
            out.flush()
        } catch (e: IOException) {
            socket.close()
            return
        }
        request.markConnected()
        TransferServer.handle(socket, sink, handler, local)
    }
}

/** Where received bytes go. Android: MediaStore Downloads; JVM/tests: a directory. */
interface FileSink {
    fun create(name: String, size: Long): PendingFile
}

interface PendingFile {
    val output: OutputStream
    /** Makes the file visible; returns a human-readable location. */
    fun commit(): String
    /** Deletes the partial file. Must be safe to call more than once. */
    fun abort()
}

/** Writes `name.hoppy-part` then renames, like the Rust core. */
class DirectorySink(private val dir: File) : FileSink {
    override fun create(name: String, size: Long): PendingFile {
        dir.mkdirs()
        val part = FileNames.uniqueFile(dir, "$name.hoppy-part")
        if (!part.createNewFile()) throw IOException("cannot create $part")
        val stream = FileOutputStream(part)
        return object : PendingFile {
            var closed = false
            override val output: OutputStream = BufferedOutputStream(stream, Protocol.CHUNK)
            override fun commit(): String {
                output.flush()
                stream.fd.sync()
                output.close()
                closed = true
                val target = FileNames.uniqueFile(dir, name)
                if (!part.renameTo(target)) throw IOException("cannot rename $part")
                return target.path
            }
            override fun abort() {
                if (!closed) runCatching { output.close() }
                closed = true
                part.delete()
            }
        }
    }
}

/** Decides whether to accept an incoming offer and observes the connection. */
interface ReceiveHandler {
    /**
     * Blocking; may ask the user. [session] says whether the connection is encrypted, the
     * sender's trust state (New/Known/Verified; KeyChanged offers never get here) and the
     * SAS to show next to the question.
     */
    fun decide(offer: Offer, from: InetAddress, session: SessionInfo): Boolean
    /**
     * true if [decide] accepted without asking a person (auto-accept): such transfers do not
     * remember the sender's key (trust on first use needs a human yes).
     */
    fun autoAccepts(): Boolean = false
    fun onProgress(from: InetAddress, done: Long, total: Long) {}
    fun onFinished(from: InetAddress, offer: Offer, saved: List<String>, error: String?) {}
    /**
     * An offer was declined without asking (spec §7.5): [reason] is `key_changed` (the
     * sender's key is not the remembered one: warn, offer "Forget device") or
     * `encryption_required` (plaintext from a device known to encrypt, or plaintext not
     * allowed). Rate-limited by the core (once a minute per device and reason).
     */
    fun onBlocked(from: InetAddress, offer: Offer, session: SessionInfo, reason: String) {}
    /** Something to tell the user (rate-limited by the core). */
    fun onWarning(from: InetAddress, warning: Warning) {}
}

object TransferServer {
    private const val IO_TIMEOUT_MS = TransferClient.IO_TIMEOUT_MS
    /** Receiver: messages 1 and 3 and the first encrypted frame within this (spec §7.2). */
    const val HANDSHAKE_DEADLINE_MS = 10_000L

    /** Handles one connection (v0 or v1, chosen by the hello byte) to completion. Never throws. */
    fun handle(socket: Socket, sink: FileSink, handler: ReceiveHandler, local: Local) {
        socket.use { s ->
            try {
                val timing = Timing(IO_TIMEOUT_MS, null)
                val input = BufferedInputStream(TimedInputStream(s, timing), Protocol.CHUNK)
                val from = s.inetAddress
                if (Protocol.readHelloVersion(input) == Protocol.VERSION_V1) {
                    receiveV1(s, input, timing, from, sink, handler, local)
                    return
                }
                val out = BufferedOutputStream(s.getOutputStream())
                val offer = Offer.from(Protocol.readJson(input))
                if (!local.allowPlaintext) {
                    reject(out, "encryption_required")
                    return
                }
                // A device whose key we hold speaks v1: plaintext from its id is a downgrade
                // or an impersonation. The specific reason stays local (spec §7.4).
                if (local.trust.get(offer.sender.id) != null) {
                    reject(out, "declined")
                    reportBlocked(local, handler, from, offer, SessionInfo.PLAINTEXT, "encryption_required")
                    return
                }
                receiveOffer(input, out, offer, SessionInfo.PLAINTEXT, from, sink, handler, local)
            } catch (e: IOException) {
                // Malformed or dropped connection before acceptance: nothing to report.
            } catch (e: RuntimeException) {
                // Malformed peer data that slipped past the parsers: drop the connection.
            }
        }
    }

    private fun reject(out: OutputStream, reason: String) {
        runCatching { Protocol.writeJson(out, mapOf("accept" to false, "reason" to reason)); out.flush() }
    }

    private fun ipKey(from: InetAddress) = from.hostAddress?.substringBefore('%') ?: "?"

    private fun reportBlocked(local: Local, handler: ReceiveHandler, from: InetAddress, offer: Offer, session: SessionInfo, reason: String) {
        if (local.guard.shouldWarn("blocked:${offer.sender.id}:$reason", HandshakeGuard.nowMs())) {
            handler.onBlocked(from, offer, session, reason)
        }
    }

    private fun reportAborts(local: Local, handler: ReceiveHandler, from: InetAddress, recent: Int) {
        if (local.guard.shouldWarn("aborts:${ipKey(from)}", HandshakeGuard.nowMs())) {
            handler.onWarning(from, Warning.HandshakeAborts(recent))
        }
    }

    private class Opened(val remote: Remote, val tx: CipherState, val input: InputStream, val first: Map<String, Any?>)

    /**
     * Responder side of spec §7.3, then the offer (§7.4: any other first frame is an invalid
     * offer). Sessions that end after message 2 without a first frame count as aborted
     * handshakes ([HandshakeGuard]).
     */
    private fun receiveV1(
        s: Socket,
        input: InputStream,
        timing: Timing,
        from: InetAddress,
        sink: FileSink,
        handler: ReceiveHandler,
        local: Local,
    ) {
        val ip = ipKey(from)
        val started = HandshakeGuard.nowMs()
        timing.set(IO_TIMEOUT_MS, started + HANDSHAKE_DEADLINE_MS)
        val refused = local.guard.admit(ip, started)
        val msg1 = Noise.readFrame(input, Noise.MAX_HANDSHAKE_MESSAGE) ?: return
        val raw = s.getOutputStream()
        if (refused != null) {
            // An empty frame instead of message 2: "busy, not now" (never read as a v0 close).
            Noise.writeFrame(raw, ByteArray(0))
            raw.flush()
            reportAborts(local, handler, from, refused)
            return
        }
        val resp = Responder(local.key)
        resp.readMsg1(msg1)
        Noise.writeFrame(raw, resp.writeMsg2(local.device))
        raw.flush()

        // From here on the sender knows this session's code: ending before the first frame
        // is an aborted handshake.
        val opened = try {
            val msg3 = Noise.readFrame(input, Noise.MAX_HANDSHAKE_MESSAGE) ?: throw EOFException()
            val remote = resp.readMsg3(msg3)
            val (tx, rx) = resp.finish()
            val noiseIn = NoiseInputStream(input, rx)
            Opened(remote, tx, noiseIn, Protocol.readJson(noiseIn))
        } catch (e: IOException) {
            null
        } catch (e: RuntimeException) {
            null
        }
        if (opened == null) {
            val recent = local.guard.recordAbort(ip, HandshakeGuard.nowMs())
            if (recent >= HandshakeGuard.MAX_ABORTS_PER_IP) reportAborts(local, handler, from, recent)
            return
        }
        timing.set(IO_TIMEOUT_MS, null)

        val peer = PeerKey.of(opened.remote)
        val state = local.trust.state(peer.id, peer.fingerprint)
        val session = SessionInfo(
            encrypted = true, peer = peer, trust = state, sas = opened.remote.sas,
            nameMatchesKnown = state == TrustState.NEW && local.trust.nameTakenByOther(peer.id, peer.name),
        )
        val out = NoiseOutputStream(raw, opened.tx)
        val first = opened.first
        val offer = try {
            Offer.from(first)
        } catch (e: ProtocolException) {
            reject(out, "invalid_offer")
            return
        }
        if (offer.sender.id != peer.id) {
            reject(out, "invalid_offer") // the offer must come from the handshake's id
            return
        }
        if (state == TrustState.KEY_CHANGED) {
            reject(out, "declined") // the specific reason stays local (spec §7.4)
            reportBlocked(local, handler, from, offer, session, "key_changed")
            return
        }
        receiveOffer(opened.input, out, offer, session, from, sink, handler, local)
    }

    /** Common to v0 and v1 once the offer is read: validate, ask, receive, answer. */
    private fun receiveOffer(
        input: InputStream,
        out: OutputStream,
        offer: Offer,
        session: SessionInfo,
        from: InetAddress,
        sink: FileSink,
        handler: ReceiveHandler,
        local: Local,
    ) {
        val reason = when {
            offer.files.isEmpty() -> "invalid_offer"
            offer.files.size > Protocol.MAX_FILES -> "too_many_files"
            else -> null
        }
        val names = offer.files.map { FileNames.sanitize(it.name) }
        if (reason != null || names.any { it == null }) {
            reject(out, reason ?: "invalid_offer")
            return
        }
        if (!handler.decide(offer, from, session)) {
            reject(out, "declined")
            return
        }
        // TOFU: remembered once a person accepted (never on automatic accepts).
        val peer = session.peer
        if (peer != null && !handler.autoAccepts()) {
            try {
                local.trust.pin(peer)
            } catch (e: IOException) {
                handler.onWarning(from, Warning.TrustNotSaved(e.message ?: "I/O error"))
            }
        }
        Protocol.writeJson(out, mapOf("accept" to true))
        out.flush()

        val saved = ArrayList<String>()
        val failed = ArrayList<String>()
        val buf = ByteArray(Protocol.CHUNK)
        val expected = ByteArray(Protocol.HASH_LEN)
        val total = offer.totalSize
        var done = 0L
        try {
            for ((entry, name) in offer.files.zip(names.map { it!! })) {
                val pending = sink.create(name, entry.size)
                try {
                    val digest = MessageDigest.getInstance("SHA-256")
                    var remaining = entry.size
                    while (remaining > 0) {
                        val n = input.read(buf, 0, minOf(remaining, buf.size.toLong()).toInt())
                        if (n < 0) throw IOException("connection closed mid-file")
                        digest.update(buf, 0, n)
                        pending.output.write(buf, 0, n)
                        remaining -= n
                        done += n
                        handler.onProgress(from, done, total)
                    }
                    readFully(input, expected)
                    if (digest.digest().contentEquals(expected)) {
                        saved += pending.commit()
                    } else {
                        failed += name
                        pending.abort()
                    }
                } catch (e: Exception) {
                    pending.abort()
                    throw e
                }
            }
        } catch (e: IOException) {
            handler.onFinished(from, offer, saved, e.message ?: "I/O error")
            return
        }
        val error = if (failed.isEmpty()) null else "hash mismatch: ${failed.joinToString(", ")}"
        Protocol.writeJson(out, if (error == null) mapOf("ok" to true) else mapOf("ok" to false, "error" to error))
        out.flush()
        handler.onFinished(from, offer, saved, error)
    }

    private fun readFully(input: InputStream, buf: ByteArray) {
        var off = 0
        while (off < buf.size) {
            val n = input.read(buf, off, buf.size - off)
            if (n < 0) throw IOException("connection closed")
            off += n
        }
    }
}
