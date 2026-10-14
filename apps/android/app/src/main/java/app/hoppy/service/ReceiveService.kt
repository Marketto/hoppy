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
import app.hoppy.core.Offer
import app.hoppy.core.Protocol
import app.hoppy.core.ReceiveHandler
import app.hoppy.core.TransferServer
import app.hoppy.ui.MainActivity
import app.hoppy.ui.Format
import java.net.InetAddress
import java.net.ServerSocket
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArraySet
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
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

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        ServiceCompat.startForeground(
            this, NOTIF_RECEIVER, receiverNotification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
        )
        Node.acquire(this)
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
        val sink = MediaStoreSink(this)
        while (running) {
            val client = runCatching { socket.accept() }.getOrNull() ?: break
            pool.execute { TransferServer.handle(client, sink, handler) }
        }
    }

    private val handler = object : ReceiveHandler {
        override fun decide(offer: Offer, from: InetAddress): Boolean {
            if (Prefs.autoAccept(this@ReceiveService)) return true
            return askUser(offer)
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
            val title = if (error == null) {
                resources.getQuantityString(R.plurals.received_files, saved.size, saved.size, offer.sender.name)
            } else {
                getString(R.string.receive_failed, offer.sender.name)
            }
            val openDownloads = PendingIntent.getActivity(
                this@ReceiveService, 0,
                Intent(DownloadManager.ACTION_VIEW_DOWNLOADS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                PendingIntent.FLAG_IMMUTABLE,
            )
            notify(
                nextId.incrementAndGet(),
                base(HoppyApp.CH_DONE)
                    .setContentTitle(title)
                    .setContentText(error ?: saved.joinToString(", ") { it.substringAfterLast('/') })
                    .setContentIntent(openDownloads)
                    .setAutoCancel(true)
                    .build(),
            )
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
        val names = offer.files.take(3).joinToString(", ") { it.name } +
            if (offer.files.size > 3) " +${offer.files.size - 3}" else ""
        notify(
            id,
            base(HoppyApp.CH_INCOMING)
                .setContentTitle(
                    resources.getQuantityString(
                        R.plurals.incoming_title, offer.files.size, offer.sender.name, offer.files.size,
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
        private val stateListeners = CopyOnWriteArraySet<() -> Unit>()
        fun addStateListener(l: () -> Unit) { stateListeners += l }
        fun removeStateListener(l: () -> Unit) { stateListeners -= l }
        private fun notifyState() = stateListeners.forEach { it() }

        fun start(ctx: Context) = ContextCompat.startForegroundService(ctx, Intent(ctx, ReceiveService::class.java))
        fun stop(ctx: Context) { ctx.stopService(Intent(ctx, ReceiveService::class.java)) }
    }
}
