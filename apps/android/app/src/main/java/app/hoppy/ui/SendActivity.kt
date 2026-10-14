package app.hoppy.ui

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.view.WindowManager
import androidx.core.content.IntentCompat
import androidx.core.view.isVisible
import app.hoppy.Node
import app.hoppy.Prefs
import app.hoppy.R
import app.hoppy.core.Peer
import app.hoppy.core.SendOutcome
import app.hoppy.core.TransferClient
import app.hoppy.databinding.ActivitySendBinding
import app.hoppy.ui.theme.ButtonFx
import app.hoppy.ui.theme.Motion
import app.hoppy.ui.theme.ThemeFonts
import kotlin.concurrent.thread

/**
 * "Share → Hoppy" and in-app sending: pick a nearby device, watch progress.
 * Entry points: the share sheet (app grid or a sharing shortcut, which carries
 * `Intent.EXTRA_SHORTCUT_ID`), and the home screen's "Send files" picker.
 * A device shortcut / tapped device is sent to as soon as discovery finds it; after
 * [PRESELECT_TIMEOUT_MS] without it the normal list is left to the user.
 * PoC limitation: the transfer runs in this screen (see docs/roadmap.md).
 */
class SendActivity : ThemedActivity() {
    private lateinit var b: ActivitySendBinding
    private val adapter = PeerAdapter { send(it) }
    private var sources: List<UriSource>? = null
    private var preselected: String? = null
    private val givingUp = Runnable { givePreselectUp() }
    @Volatile private var sending = false

    private val peersListener: (List<Peer>) -> Unit = { peers -> runOnUiThread { showPeers(peers) } }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        b = ActivitySendBinding.inflate(layoutInflater)
        setContentView(b.root)
        b.peers.adapter = adapter
        b.close.setOnClickListener { finish() }
        b.summary.setText(R.string.preparing)
        if (savedInstanceState == null) preselect(intent)
        ThemeFonts.applyDisplay(b.delivered, b.sendToHeader)
        ThemeFonts.applyBodyBold(b.summary)

        val uris = incomingUris(intent)
        val text = intent.getStringExtra(Intent.EXTRA_TEXT)
        thread(name = "hoppy-resolve") {
            val resolved = runCatching {
                when {
                    uris.isNotEmpty() -> uris.map { UriSource.resolve(this, it) }
                    !text.isNullOrBlank() -> listOf(UriSource.fromText(this, text))
                    else -> emptyList()
                }
            }.getOrElse { emptyList() }
            runOnUiThread { onResolved(resolved) }
        }
    }

    /** Device chosen before this screen opened: a sharing shortcut or a tapped device. */
    private fun preselect(intent: Intent) {
        val shortcutId = intent.getStringExtra(Intent.EXTRA_SHORTCUT_ID)
        if (shortcutId != null) thread(name = "hoppy-shortcut") { ShareShortcuts.reportUsed(this, shortcutId) }
        val peerId = intent.getStringExtra(EXTRA_PEER)?.takeIf(ShareTargets::isValidPeerId)
            ?: ShareTargets.peerIdOf(shortcutId)
            ?: return
        preselected = peerId
        val name = Node.peers().firstOrNull { it.device.id == peerId }?.device?.name
            ?: ShareShortcuts.recentName(this, peerId)
        if (name != null) {
            b.status.text = getString(R.string.looking_for, name)
            b.status.isVisible = true
        }
        b.root.postDelayed(givingUp, PRESELECT_TIMEOUT_MS)
    }

    /** Not found in time: forget it, so a late discovery never starts an unexpected send. */
    private fun givePreselectUp() {
        val id = preselected ?: return
        // Already seen but files still being prepared: keep it, onResolved() sends.
        if (Node.peers().any { it.device.id == id }) return
        preselected = null
        if (!sending) b.status.isVisible = false
    }

    override fun onDestroy() {
        if (::b.isInitialized) b.root.removeCallbacks(givingUp)
        super.onDestroy()
    }

    override fun onStart() {
        super.onStart()
        Node.addListener(peersListener)
        Node.acquire(this)
        Node.refresh()
        showPeers(Node.peers())
    }

    override fun onStop() {
        Node.removeListener(peersListener)
        Node.release()
        super.onStop()
    }

    private fun onResolved(list: List<UriSource>) {
        if (list.isEmpty()) {
            b.summary.setText(R.string.nothing_to_send)
            return
        }
        sources = list
        b.summary.text = resources.getQuantityString(
            R.plurals.send_summary, list.size, list.size, Format.size(list.sumOf { it.size }),
        )
        showPeers(Node.peers())
    }

    private fun showPeers(peers: List<Peer>) {
        adapter.submitList(peers)
        b.empty.isVisible = peers.isEmpty() && !sending
        val target = preselected?.let { id -> peers.firstOrNull { it.device.id == id } }
        if (target != null && sources != null && !sending) {
            preselected = null
            send(target)
        }
    }

    private fun send(peer: Peer) {
        val list = sources ?: return
        if (sending) return
        sending = true
        adapter.enabled = false
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        // Switch mode while hidden: some Material versions throw when switching a
        // visible indicator to indeterminate.
        b.progress.isIndeterminate = true
        b.progress.isVisible = true
        b.status.isVisible = true
        b.status.text = getString(R.string.waiting_accept, peer.device.name)
        val me = Prefs.device(this)
        var lastUi = 0L
        thread(name = "hoppy-send") {
            val result = runCatching {
                TransferClient.send(peer.address, peer.port, me, list) { done, total ->
                    val now = System.currentTimeMillis()
                    if (now - lastUi > 150 || done == total) {
                        lastUi = now
                        val pct = if (total == 0L) 100 else (done * 100 / total).toInt()
                        runOnUiThread {
                            b.progress.isIndeterminate = false
                            b.progress.setProgressCompat(pct, true)
                            b.status.text = getString(R.string.sending_progress, peer.device.name, pct)
                        }
                    }
                }
            }
            runOnUiThread { onSendResult(peer, result) }
        }
    }

    private fun onSendResult(peer: Peer, result: Result<SendOutcome>) {
        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val outcome = result.getOrNull()
        if (outcome == SendOutcome.Delivered) {
            val app = applicationContext
            thread(name = "hoppy-shortcuts") { ShareShortcuts.onDelivered(app, peer.device.id, peer.device.name) }
            b.progress.setProgressCompat(100, true)
            b.status.text = getString(R.string.delivered, peer.device.name)
            b.mascot.celebrate()
            b.delivered.isVisible = true
            if (Motion.enabled()) ButtonFx.popIn(b.delivered).start()
            b.root.postDelayed({ if (!isFinishing) finish() }, 1800)
            return
        }
        sending = false
        adapter.enabled = true
        b.progress.isVisible = false
        b.status.text = when (outcome) {
            is SendOutcome.Declined -> getString(R.string.declined, peer.device.name)
            else -> getString(R.string.send_failed, result.exceptionOrNull()?.message ?: "?")
        }
    }

    companion object {
        private const val EXTRA_PEER = "app.hoppy.PEER_ID"
        private const val PRESELECT_TIMEOUT_MS = 6_000L

        /**
         * In-app sending (home "Send files" picker): the same ACTION_SEND(_MULTIPLE)
         * shape the share sheet uses, with the read grant carried by ClipData.
         * [uris] must not be empty.
         */
        fun intent(ctx: Context, uris: List<Uri>, peerId: String?): Intent {
            val i = Intent(ctx, SendActivity::class.java).setType("*/*")
            if (uris.size == 1) {
                i.action = Intent.ACTION_SEND
                i.putExtra(Intent.EXTRA_STREAM, uris[0])
            } else {
                i.action = Intent.ACTION_SEND_MULTIPLE
                i.putParcelableArrayListExtra(Intent.EXTRA_STREAM, ArrayList(uris))
            }
            // newRawUri: no ContentResolver query on the main thread.
            val clip = ClipData.newRawUri(null, uris[0])
            uris.drop(1).forEach { clip.addItem(ClipData.Item(it)) }
            i.clipData = clip
            i.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            if (peerId != null) i.putExtra(EXTRA_PEER, peerId)
            return i
        }

        private fun incomingUris(intent: Intent): List<Uri> = when (intent.action) {
            Intent.ACTION_SEND -> listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
            Intent.ACTION_SEND_MULTIPLE ->
                IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            else -> emptyList()
        }
    }
}
