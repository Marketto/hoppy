package app.hoppy.core

import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
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

fun interface ProgressListener {
    fun onProgress(done: Long, total: Long)
}

object TransferClient {
    private const val CONNECT_TIMEOUT_MS = 5_000
    /** Direct attempt before falling back to a callback (spec §6). */
    const val DIRECT_CONNECT_TIMEOUT_MS = 3_000
    /** One deadline for the whole callback hello, not per read (spec §6 step 3). */
    private const val CALLBACK_HELLO_TIMEOUT_MS = 3_000L
    private const val DECISION_TIMEOUT_MS = 120_000 // a human may be deciding
    private const val IO_TIMEOUT_MS = 30_000

    /**
     * Throws [IOException] on network/remote errors; [PeerUnreachableException] if the
     * connection cannot even be opened. Spec §3.
     */
    fun send(
        host: InetAddress,
        port: Int,
        me: DeviceInfo,
        sources: List<Source>,
        progress: ProgressListener = ProgressListener { _, _ -> },
    ): SendOutcome {
        require(sources.isNotEmpty()) { "nothing to send" }
        Socket().use { socket ->
            try {
                socket.connect(InetSocketAddress(host, port), CONNECT_TIMEOUT_MS)
            } catch (e: IOException) {
                throw PeerUnreachableException(e)
            }
            return sendOver(socket, me, sources, progress)
        }
    }

    /**
     * Sends to a discovered [peer]: direct TCP first (3 s); if that cannot connect and [node]
     * is given, asks the peer to connect back (spec §6, for receivers behind an inbound
     * firewall). Blocks up to ~23 s before [PeerUnreachableException]. Not on the main thread.
     * [onFallback] runs (on this thread) exactly when the direct attempt failed and callback
     * mode starts, like Rust's `Progress::Fallback`; never for a direct send.
     */
    fun sendToPeer(
        peer: Peer,
        me: DeviceInfo,
        sources: List<Source>,
        node: DiscoveryNode?,
        onFallback: () -> Unit = {},
        progress: ProgressListener = ProgressListener { _, _ -> },
    ): SendOutcome {
        require(sources.isNotEmpty()) { "nothing to send" }
        val direct = Socket()
        try {
            direct.connect(InetSocketAddress(peer.address, peer.port), DIRECT_CONNECT_TIMEOUT_MS)
        } catch (e: IOException) {
            direct.close()
            if (node == null) throw PeerUnreachableException(e)
            return sendViaCallback(peer, me, sources, node, onFallback, progress)
        }
        direct.use { return sendOver(it, me, sources, progress) }
    }

    /**
     * Callback only (spec §6); [sendToPeer] calls this when the direct connect fails.
     * [onFallback] runs first, before the callback request goes out.
     */
    fun sendViaCallback(
        peer: Peer,
        me: DeviceInfo,
        sources: List<Source>,
        node: DiscoveryNode,
        onFallback: () -> Unit = {},
        progress: ProgressListener = ProgressListener { _, _ -> },
    ): SendOutcome {
        require(sources.isNotEmpty()) { "nothing to send" }
        onFallback()
        connectViaCallback(peer, node).use { return sendOver(it, me, sources, progress) }
    }

    private fun connectViaCallback(peer: Peer, node: DiscoveryNode): Socket {
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
                            val (id, got) = Protocol.readCallbackHello(input) // unbuffered: no read-ahead
                            if (Protocol.tokensEqual(got, token) && id == peer.device.id) return client
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

    /** Spec §3 from the sender's side over an open connection (direct or callback). */
    private fun sendOver(socket: Socket, me: DeviceInfo, sources: List<Source>, progress: ProgressListener): SendOutcome {
        val offer = Offer(me, sources.map { FileEntry(it.name, it.size) })
        val total = offer.totalSize
        socket.use {
            socket.tcpNoDelay = true
            val input = BufferedInputStream(socket.getInputStream())
            val out = BufferedOutputStream(socket.getOutputStream(), Protocol.CHUNK)

            Protocol.writeHello(out)
            Protocol.writeJson(out, offer.toMap())
            out.flush()

            socket.soTimeout = DECISION_TIMEOUT_MS
            val decision = Protocol.readJson(input)
            if (!decision.bool("accept")) return SendOutcome.Declined(decision["reason"] as? String)
            socket.soTimeout = IO_TIMEOUT_MS

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
                        progress.onProgress(done, total)
                    }
                }
                out.write(digest.digest())
            }
            out.flush()

            val result = Protocol.readJson(input)
            if (result.bool("ok")) return SendOutcome.Delivered
            throw IOException("receiver error: ${result["error"] as? String ?: "unknown"}")
        }
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
    fun install(node: DiscoveryNode, sink: FileSink, handler: ReceiveHandler) {
        node.callbackHandler = { request -> answer(request, node.me, sink, handler) }
    }

    /**
     * Connects to the requester, sends the callback hello, then serves the transfer. Never
     * throws. [onSocket] gets the socket before connecting, so the caller can close it to
     * cancel (e.g. when the Android service is destroyed).
     */
    fun answer(
        request: CallbackRequest,
        me: DeviceInfo,
        sink: FileSink,
        handler: ReceiveHandler,
        onSocket: (Socket) -> Unit = {},
    ) {
        val socket = Socket()
        try {
            onSocket(socket)
            socket.connect(InetSocketAddress(request.address, request.port), DiscoveryNode.CALLBACK_CONNECT_TIMEOUT_MS)
            val out = socket.getOutputStream()
            Protocol.writeCallbackHello(out, me.id, request.token)
            out.flush()
        } catch (e: IOException) {
            socket.close()
            return
        }
        request.markConnected()
        TransferServer.handle(socket, sink, handler)
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

interface ReceiveHandler {
    /** Blocking; may ask the user. */
    fun decide(offer: Offer, from: InetAddress): Boolean
    fun onProgress(from: InetAddress, done: Long, total: Long) {}
    fun onFinished(from: InetAddress, offer: Offer, saved: List<String>, error: String?) {}
}

object TransferServer {
    private const val IO_TIMEOUT_MS = 30_000

    /** Handles one connection to completion. Never throws. */
    fun handle(socket: Socket, sink: FileSink, handler: ReceiveHandler) {
        socket.use { s ->
            try {
                s.soTimeout = IO_TIMEOUT_MS
                val input = BufferedInputStream(s.getInputStream(), Protocol.CHUNK)
                val out = BufferedOutputStream(s.getOutputStream())
                val from = s.inetAddress
                Protocol.readHello(input)
                val offer = Offer.from(Protocol.readJson(input))

                val reject = when {
                    offer.files.isEmpty() -> "invalid_offer"
                    offer.files.size > Protocol.MAX_FILES -> "too_many_files"
                    else -> null
                }
                val names = offer.files.map { FileNames.sanitize(it.name) }
                if (reject != null || names.any { it == null }) {
                    Protocol.writeJson(out, mapOf("accept" to false, "reason" to (reject ?: "invalid_offer")))
                    return
                }
                if (!handler.decide(offer, from)) {
                    Protocol.writeJson(out, mapOf("accept" to false, "reason" to "declined"))
                    return
                }
                Protocol.writeJson(out, mapOf("accept" to true))

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
                handler.onFinished(from, offer, saved, error)
            } catch (e: IOException) {
                // Malformed or dropped connection before acceptance: nothing to report.
            }
        }
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
