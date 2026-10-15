package app.hoppy.service

import android.app.DownloadManager
import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import app.hoppy.HoppyApp
import app.hoppy.Node
import app.hoppy.Prefs
import app.hoppy.R
import app.hoppy.core.CallbackReceiver
import app.hoppy.core.Offer
import app.hoppy.core.Protocol
import app.hoppy.core.ReceiveHandler
import app.hoppy.core.TransferServer
import app.hoppy.ui.MainActivity
import app.hoppy.ui.Format
import app.hoppy.ui.SafeText
import java.net.InetAddress
import java.net.ServerSocket
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArraySet
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlin.concurrent.thread

/**
 * Keeps the device visible and accepts transfers while enabled.
 * PoC uses the dataSync foreground type; see docs/platforms/android.md for limits.
 */
class ReceiveService : Service() {
    private var server: ServerSocket? = null
    private val pool: ExecutorService = Executors.newCachedThreadPool()
    @Volatile private var lastProgressUpdate = 0L
    /** An Accept/Decline prompt is showing (direct or callback offer). */
    private val deciding = AtomicBoolean(false)
    private val sink by lazy { MediaStoreSink(this) }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        ServiceCompat.startForeground(
            this, NOTIF_RECEIVER, receiverNotification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
        )
        Node.acquire(this)
        // Senders that cannot reach our TCP port ask us to connect back (spec §6); the
        // node only honours this while receivePort != 0. Same sink and decision path.
        val me = Prefs.device(this)
        Node.callbackHandler = { request -> serve { CallbackReceiver.answer(request, me, sink, it) } }
        running = true
        notifyState()
        thread(name = "hoppy-receive-accept", isDaemon = true) { acceptLoop() }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) stopSelf()
        // Not sticky: a system restart happens in the background, where starting a
        // dataSync foreground service throws ForegroundServiceStartNotAllowedException.
        return START_NOT_STICKY
    }

    /** Android 15: dataSync services get ~6 h; not stopping here crashes the app. */
    override fun onTimeout(startId: Int, fgsType: Int) {
        stopSelf()
    }

    override fun onDestroy() {
        running = false
        runCatching { server?.close() }
        Node.callbackHandler = null
        Node.receivePort = 0
        Node.release()
        pool.shutdownNow()
        notifyState()
        super.onDestroy()
    }

    private fun acceptLoop() {
        val socket = runCatching { ServerSocket(Protocol.TRANSFER_PORT) }.getOrNull()
            ?: runCatching { ServerSocket(0) }.getOrNull()
            ?: run { stopSelf(); return }
        server = socket
        Node.receivePort = socket.localPort
        while (running) {
            val client = runCatching { socket.accept() }.getOrNull() ?: break
            pool.execute { serve { TransferServer.handle(client, sink, it) } }
        }
    }

    /**
     * One incoming transfer, direct or connected back (callback): same accept prompt,
     * progress, end-of-receive notification and [receiving] flag. Blocks until it ends.
     */
    private fun serve(transfer: (Connection) -> Unit) {
        val connection = Connection()
        try {
            transfer(connection)
        } finally {
            if (connection.accepted) receiveEnded()
        }
    }

    /** One incoming connection; [accepted] marks it as an active receive (music stays off). */
    private inner class Connection : ReceiveHandler {
        @Volatile var accepted = false

        override fun decide(offer: Offer, from: InetAddress): Boolean {
            val ok = Prefs.autoAccept(this@ReceiveService) || askOneAtATime(offer)
            if (ok) {
                accepted = true
                receiveStarted()
            }
            return ok
        }

        override fun onProgress(from: InetAddress, done: Long, total: Long) {
            val now = System.currentTimeMillis()
            if (now - lastProgressUpdate < 400 && done != total) return
            lastProgressUpdate = now
            val pct = if (total == 0L) 100 else (done * 100 / total).toInt()
            notify(
                NOTIF_RECEIVER,
                base(HoppyApp.CH_RECEIVER)
                    .setContentTitle(getString(R.string.receiving_progress, pct))
                    .setProgress(100, pct, false)
                    .setOngoing(true)
                    .build(),
            )
        }

        override fun onFinished(from: InetAddress, offer: Offer, saved: List<String>, error: String?) {
            notify(NOTIF_RECEIVER, receiverNotification())
            notifyFinished(offer, saved, error)
        }
    }

    /**
     * System notification at the end of every accepted receive ("Received files" channel,
     * default sound), one id per transfer. Tap: the file itself when there is exactly one
     * with a viewable type, otherwise the Downloads app (or Hoppy when nothing was saved).
     */
    private fun notifyFinished(offer: Offer, saved: List<String>, error: String?) {
        val uris = saved.map { sink.takeUri(it) }
        val id = nextId.incrementAndGet()
        val sender = SafeText.label(offer.sender.name)
        val title = if (error == null) {
            resources.getQuantityString(R.plurals.received_files, saved.size, saved.size, sender)
        } else {
            getString(R.string.receive_failed, sender)
        }
        val single = uris.singleOrNull()?.takeIf { error == null }
        val type = single?.let { runCatching { contentResolver.getType(it) }.getOrNull() }
        val open = when {
            single != null && type != null && type != "application/octet-stream" ->
                Intent(Intent.ACTION_VIEW)
                    .setDataAndType(single, type)
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
            saved.isNotEmpty() ->
                Intent(DownloadManager.ACTION_VIEW_DOWNLOADS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            else -> Intent(this, MainActivity::class.java)
        }
        val tap = PendingIntent.getActivity(
            this, id, open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val text = error?.let { SafeText.label(it, 200) }
            ?: saved.joinToString(", ") { SafeText.label(it.substringAfterLast('/'), 200) }
        notify(
            id,
            base(HoppyApp.CH_DONE)
                .setContentTitle(title)
                .setContentText(text)
                .setStyle(NotificationCompat.BigTextStyle().bigText(text))
                .setCategory(if (error == null) NotificationCompat.CATEGORY_STATUS else NotificationCompat.CATEGORY_ERROR)
                .setContentIntent(tap)
                .setAutoCancel(true)
                .build(),
        )
    }

    /** One prompt at a time: an offer arriving while another awaits a decision is declined. */
    private fun askOneAtATime(offer: Offer): Boolean {
        if (!deciding.compareAndSet(false, true)) return false
        try {
            return askUser(offer)
        } finally {
            deciding.set(false)
        }
    }

    /** Shows Accept/Decline actions and blocks this connection's thread for up to 60 s. */
    private fun askUser(offer: Offer): Boolean {
        if (!NotificationManagerCompat.from(this).areNotificationsEnabled()) return false
        val id = nextId.incrementAndGet()
        val future = CompletableFuture<Boolean>()
        pending[id] = future
        fun action(accept: Boolean) = PendingIntent.getBroadcast(
            this, id * 2 + if (accept) 1 else 0,
            Intent(this, DecisionReceiver::class.java)
                .putExtra(DecisionReceiver.EXTRA_ID, id)
                .putExtra(DecisionReceiver.EXTRA_ACCEPT, accept),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        // Peer-supplied: show names as they would be saved (§4) and without bidi tricks.
        val names = offer.files.take(3).joinToString(", ") { SafeText.offeredFileName(it.name) } +
            if (offer.files.size > 3) " +${offer.files.size - 3}" else ""
        val sender = SafeText.label(offer.sender.name)
        notify(
            id,
            base(HoppyApp.CH_INCOMING)
                .setContentTitle(
                    resources.getQuantityString(
                        R.plurals.incoming_title, offer.files.size, sender, offer.files.size,
                        Format.size(offer.totalSize),
                    ),
                )
                .setContentText(names)
                .setPriority(NotificationCompat.PRIORITY_HIGH)
                .setCategory(NotificationCompat.CATEGORY_CALL)
                .setTimeoutAfter(DECISION_TIMEOUT_S * 1000)
                .addAction(0, getString(R.string.decline), action(false))
                .addAction(0, getString(R.string.accept), action(true))
                .build(),
        )
        return try {
            future.get(DECISION_TIMEOUT_S, TimeUnit.SECONDS)
        } catch (e: Exception) {
            false
        } finally {
            pending.remove(id)
            getSystemService(NotificationManager::class.java).cancel(id)
        }
    }

    private fun base(channel: String) = NotificationCompat.Builder(this, channel)
        .setSmallIcon(R.drawable.ic_notification)

    private fun receiverNotification(): Notification {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
        )
        val stop = PendingIntent.getService(
            this, 1, Intent(this, ReceiveService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE,
        )
        return base(HoppyApp.CH_RECEIVER)
            .setContentTitle(getString(R.string.receiver_on, Prefs.deviceName(this)))
            .setContentText(getString(R.string.receiver_on_detail))
            .setContentIntent(open)
            .addAction(0, getString(R.string.stop), stop)
            .setOngoing(true)
            .build()
    }

    private fun notify(id: Int, n: Notification) {
        runCatching { NotificationManagerCompat.from(this).notify(id, n) } // SecurityException if denied
    }

    companion object {
        private const val ACTION_STOP = "app.hoppy.STOP"
        private const val NOTIF_RECEIVER = 1
        private const val DECISION_TIMEOUT_S = 60L
        private val nextId = AtomicInteger(100)
        internal val pending = ConcurrentHashMap<Int, CompletableFuture<Boolean>>()

        @Volatile var running = false
            private set

        /** Accepted transfers in progress (offer accepted → connection closed). */
        private val activeReceives = AtomicInteger(0)

        /** True while this device is receiving: theme music stays silent (`Music`). */
        val receiving: Boolean get() = activeReceives.get() > 0

        private fun receiveStarted() {
            activeReceives.incrementAndGet()
            notifyState()
        }

        private fun receiveEnded() {
            activeReceives.decrementAndGet()
            notifyState()
        }
        private val stateListeners = CopyOnWriteArraySet<() -> Unit>()
        fun addStateListener(l: () -> Unit) { stateListeners += l }
        fun removeStateListener(l: () -> Unit) { stateListeners -= l }
        private fun notifyState() = stateListeners.forEach { it() }

        fun start(ctx: Context) = ContextCompat.startForegroundService(ctx, Intent(ctx, ReceiveService::class.java))
        fun stop(ctx: Context) { ctx.stopService(Intent(ctx, ReceiveService::class.java)) }
    }
}
