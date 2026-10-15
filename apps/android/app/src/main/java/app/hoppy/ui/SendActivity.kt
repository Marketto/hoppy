package app.hoppy.ui

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.util.Log
import android.view.WindowManager
import android.widget.Toast
import androidx.core.content.IntentCompat
import androidx.core.view.isVisible
import app.hoppy.Node
import app.hoppy.Prefs
import app.hoppy.R
import app.hoppy.audio.Music
import app.hoppy.core.KeyChangedException
import app.hoppy.core.Peer
import app.hoppy.core.PeerKey
import app.hoppy.core.PeerUnreachableException
import app.hoppy.core.ReceiverBusyException
import app.hoppy.core.SendListener
import app.hoppy.core.SendOutcome
import app.hoppy.core.SessionInfo
import app.hoppy.core.TransferClient
import app.hoppy.core.TrustState
import app.hoppy.core.UnencryptedException
import app.hoppy.core.Warning
import app.hoppy.databinding.ActivitySendBinding
import app.hoppy.security.Security
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import app.hoppy.ui.theme.ButtonFx
import app.hoppy.ui.theme.Motion
import app.hoppy.ui.theme.ThemeFonts
import java.net.ConnectException
import java.net.NoRouteToHostException
import java.util.UUID
import kotlin.concurrent.thread

/**
 * "Share → Hoppy" and in-app sending: pick a nearby device, watch progress.
 * Entry points: the share sheet (app grid or a sharing shortcut, which carries
 * `Intent.EXTRA_SHORTCUT_ID`), and the home screen's "Send files" picker.
 * A device shortcut / device tapped on home is only *preselected*: once discovery finds
 * it, it is framed at the top of the list and a primary "Send to <name> <IP>" button
 * appears; nothing is sent without a tap (this activity is exported, any app can start
 * it). Preselection is honoured only for recent targets (devices already sent to) or for
 * our own home screen ([intent] carries a per-process token). After
 * [PRESELECT_TIMEOUT_MS] without it the normal list is left to the user.
 * Every session to a v1 device is encrypted (docs/protocol.md §7): the line under the status
 * says so ("⚠ not encrypted" for old apps); for a New/Known device the code (4 large emoji,
 * the only verification method) shows over the list while waiting for the receiver, with
 * "They match" / "Later". A changed key blocks the send with a dialog
 * offering "Forget device".
 * PoC limitation: the transfer runs in this screen (see docs/roadmap.md).
 */
class SendActivity : ThemedActivity() {
    private lateinit var b: ActivitySendBinding
    private val adapter = PeerAdapter { send(it) }
    private var sources: List<UriSource>? = null
    /** Peer id the user is offered to confirm with one tap; never sent to automatically. */
    private var preselected: String? = null
    /** The status line shows "Looking for <name>…". */
    private var lookingFor = false
    private val givingUp = Runnable { givePreselectUp() }
    @Volatile private var sending = false
    private lateinit var panel: VerifyPanel
    /** Delivered while the code was still on screen: close once the user is done with it. */
    private var delivered = false

    private val peersListener: (List<Peer>) -> Unit = { peers -> runOnUiThread { showPeers(peers) } }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        b = ActivitySendBinding.inflate(layoutInflater)
        setContentView(b.root)
        b.peers.adapter = adapter
        b.close.setOnClickListener { finish() }
        b.confirm.maxLines = 2
        b.summary.setText(R.string.preparing)
        if (savedInstanceState == null) preselect(intent)
        ThemeFonts.applyDisplay(b.delivered, b.sendToHeader)
        ThemeFonts.applyBodyBold(b.summary)
        panel = VerifyPanel(
            b.verify,
            object : VerifyPanel.Actions {
                override fun onMatch(peer: PeerKey) {
                    VerifyPanel.markVerified(this@SendActivity, peer) { ok, message -> onVerifyResult(ok, message) }
                }
                override fun onLater() = hideVerify()
            },
        )

        val uris = incomingUris(this, intent)
        // Files shared but all refused: send nothing rather than the caption text.
        val text = intent.getStringExtra(Intent.EXTRA_TEXT)?.takeUnless { intent.hasExtra(Intent.EXTRA_STREAM) }
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
        val requested = intent.getStringExtra(EXTRA_PEER) ?: ShareTargets.peerIdOf(shortcutId) ?: return
        val fromHome = intent.getStringExtra(EXTRA_TOKEN) == inAppToken
        val peerId = if (fromHome) {
            requested.takeIf(ShareTargets::isValidPeerId)
        } else {
            ShareTargets.recentPeer(requested, Prefs.recentTargets(this))
        } ?: return
        preselected = peerId
        adapter.highlightId = peerId
        val name = Node.peers().firstOrNull { it.device.id == peerId }?.device?.name
            ?: ShareShortcuts.recentName(this, peerId)
        if (name != null) {
            b.status.text = getString(R.string.looking_for, SafeText.label(name))
            b.status.isVisible = true
            lookingFor = true
        }
        b.root.postDelayed(givingUp, PRESELECT_TIMEOUT_MS)
    }

    /** Not found in time: forget it, so a late discovery never reorders the list under the user's finger. */
    private fun givePreselectUp() {
        val id = preselected ?: return
        // Already found: keep offering it (the user still has to tap).
        if (Node.peers().any { it.device.id == id }) return
        preselected = null
        adapter.highlightId = null
        lookingFor = false
        if (!sending) b.status.isVisible = false
    }

    override fun onDestroy() {
        if (::b.isInitialized) b.root.removeCallbacks(givingUp)
        // A running send still needs the cache copies; its thread cleans up when done.
        if (isFinishing && !sending) sources?.let { list -> thread(name = "hoppy-cleanup") { list.forEach { it.cleanup() } } }
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
        val target = preselected?.let { id -> peers.firstOrNull { it.device.id == id } }
        adapter.submitList(if (target == null) peers else listOf(target) + (peers - target))
        b.empty.isVisible = peers.isEmpty() && !sending && !b.verifyScroll.isVisible
        refreshTrustBadges(this, adapter)
        if (target != null && lookingFor) {
            lookingFor = false
            if (!sending) b.status.isVisible = false
        }
        val offer = target != null && sources != null && !sending
        if (target != null && offer) {
            val full = SafeText.label(target.device.name)
            val short = SafeText.label(full, CONFIRM_NAME_MAX)
            val name = if (short.length < full.length) "$short…" else short
            b.confirm.text = getString(R.string.send_to_device, name, target.address.hostAddress ?: "?")
            b.confirm.setOnClickListener { send(target) }
        }
        b.confirm.isVisible = offer
    }

    private fun send(peer: Peer) {
        val list = sources ?: return
        if (sending) return
        sending = true
        preselected = null
        lookingFor = false
        b.root.removeCallbacks(givingUp)
        b.confirm.isVisible = false
        adapter.enabled = false
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        Music.transferStarted() // faster loop on the next bar; the receiver stays silent
        // Switch mode while hidden: some Material versions throw when switching a
        // visible indicator to indeterminate.
        b.progress.isIndeterminate = true
        b.progress.isVisible = true
        b.status.isVisible = true
        b.status.text = getString(R.string.waiting_accept, SafeText.label(peer.device.name))
        b.security.isVisible = false
        val app = applicationContext
        // Keep the discovery node (UDP 47470) alive for the callback even if this screen stops.
        Node.acquire(app)
        val node = Node.node()
        var lastUi = 0L
        val listener = object : SendListener {
            // Direct TCP failed, asking the peer to connect back (spec §6): say so.
            override fun onFallback() {
                runOnUiThread {
                    if (sending) b.status.text = getString(R.string.send_trying_callback, SafeText.label(peer.device.name))
                }
            }

            override fun onSession(info: SessionInfo) {
                runOnUiThread { showSession(peer, info) }
            }

            override fun onProgress(done: Long, total: Long) {
                val now = System.currentTimeMillis()
                if (now - lastUi > 150 || done == total) {
                    lastUi = now
                    val pct = if (total == 0L) 100 else (done * 100 / total).toInt()
                    runOnUiThread {
                        b.progress.isIndeterminate = false
                        b.progress.setProgressCompat(pct, true)
                        b.status.text = getString(R.string.sending_progress, SafeText.label(peer.device.name), pct)
                    }
                }
            }

            override fun onWarning(warning: Warning) {
                if (warning is Warning.TrustNotSaved) {
                    runOnUiThread {
                        Toast.makeText(app, getString(R.string.trust_not_saved, SafeText.label(warning.message, 200)), Toast.LENGTH_LONG).show()
                    }
                }
            }
        }
        thread(name = "hoppy-send") {
            val result = try {
                // Reads the key / trust store on first use: off the main thread.
                runCatching { TransferClient.sendToPeer(peer, Security.local(app), list, node, listener) }
            } finally {
                Node.release()
            }
            if (result.getOrNull() == SendOutcome.Delivered || isFinishing || isDestroyed) list.forEach { it.cleanup() }
            runOnUiThread { onSendResult(peer, result) }
        }
    }

    private fun onSendResult(peer: Peer, result: Result<SendOutcome>) {
        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val outcome = result.getOrNull()
        Music.transferEnded(delivered = outcome == SendOutcome.Delivered)
        if (outcome == SendOutcome.Delivered) {
            val app = applicationContext
            thread(name = "hoppy-shortcuts") { ShareShortcuts.onDelivered(app, peer.device.id, peer.device.name) }
            b.progress.setProgressCompat(100, true)
            b.status.text = getString(R.string.delivered, SafeText.label(peer.device.name))
            b.mascot.celebrate()
            b.delivered.isVisible = true
            if (Motion.enabled()) ButtonFx.popIn(b.delivered).start()
            delivered = true
            // Leave the code on screen until the user answers it ("Later" or ✓ closes).
            if (!verifyPending()) b.root.postDelayed({ if (!isFinishing) finish() }, 1800)
            return
        }
        hideVerify()
        sending = false
        adapter.enabled = true
        adapter.highlightId = null
        b.progress.isVisible = false
        val error = result.exceptionOrNull()
        if (error != null) Log.w(TAG, "send to ${peer.address.hostAddress}:${peer.port} failed", error)
        val name = SafeText.label(peer.device.name)
        b.status.text = when {
            outcome is SendOutcome.Declined -> getString(R.string.declined, name)
            error is KeyChangedException -> {
                showKeyChanged(name, error.peer)
                getString(R.string.key_changed_title)
            }
            // "Busy": the receiver is limiting aborted handshakes (spec §7.3): never plaintext.
            error is ReceiverBusyException -> getString(R.string.send_busy, name)
            error is UnencryptedException -> getString(R.string.send_downgrade, name)
            // Usually the PC firewall or another network: say what to do, keep details in logcat.
            isUnreachable(error) -> getString(R.string.send_unreachable, SafeText.label(peer.device.name))
            else -> getString(R.string.send_failed, error?.message ?: "?")
        }
    }

    /** How the session is protected (before the offer goes out): line + code for unverified devices. */
    private fun showSession(peer: Peer, info: SessionInfo) {
        b.security.isVisible = true
        when {
            !info.encrypted -> b.security.text = getString(R.string.send_not_encrypted, SafeText.label(peer.device.name))
            info.trust == TrustState.VERIFIED -> b.security.text = getString(R.string.send_encrypted_verified)
            else -> {
                b.security.text = getString(R.string.send_encrypted)
                if (info.sas != null && info.peer != null) {
                    b.peers.isVisible = false
                    b.empty.isVisible = false
                    b.verifyScroll.isVisible = true
                    panel.show(info)
                }
            }
        }
    }

    private fun verifyPending(): Boolean = b.verifyScroll.isVisible && b.verify.verifyActions.isVisible

    private fun hideVerify() {
        b.verifyScroll.isVisible = false
        b.peers.isVisible = true
        if (delivered && !isFinishing) finish()
    }

    private fun onVerifyResult(ok: Boolean, message: String) {
        if (!ok) {
            Toast.makeText(this, message, Toast.LENGTH_LONG).show()
            return
        }
        panel.showDone(message)
        b.security.text = getString(R.string.send_encrypted_verified)
        refreshTrustBadges(this, adapter)
        if (delivered) b.root.postDelayed({ if (!isFinishing) finish() }, 1800)
    }

    /** The receiver's key is not the remembered one: nothing was sent (spec §7.5). */
    private fun showKeyChanged(name: String, peer: PeerKey) {
        MaterialAlertDialogBuilder(this)
            .setTitle(R.string.key_changed_title)
            .setMessage(getString(R.string.key_changed_message, name))
            .setNegativeButton(android.R.string.cancel, null)
            .setPositiveButton(R.string.forget_device) { _, _ ->
                val app = applicationContext
                thread(name = "hoppy-forget") {
                    val error = runCatching { Security.trust(app).forget(peer.id) }.exceptionOrNull()
                    runOnUiThread {
                        if (isFinishing || isDestroyed) return@runOnUiThread
                        b.status.text = if (error == null) {
                            getString(R.string.forgotten, name)
                        } else {
                            getString(R.string.forget_failed, name, error.message ?: "?")
                        }
                        refreshTrustBadges(this, adapter)
                    }
                }
            }
            .show()
    }

    /** Connection never opened: connect timeout / refused / no route (not a mid-transfer timeout). */
    private fun isUnreachable(error: Throwable?): Boolean =
        error is PeerUnreachableException || error is ConnectException || error is NoRouteToHostException

    companion object {
        private const val TAG = "HoppySend"
        private const val EXTRA_PEER = "app.hoppy.PEER_ID"
        private const val EXTRA_TOKEN = "app.hoppy.TOKEN"
        private const val PRESELECT_TIMEOUT_MS = 6_000L
        private const val CONFIRM_NAME_MAX = 24

        /**
         * Marks intents built by [intent] in this process: other apps cannot know it, so
         * their EXTRA_PEER is honoured only for recent targets. Still only a preselection.
         */
        private val inAppToken = UUID.randomUUID().toString()

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
            if (peerId != null) i.putExtra(EXTRA_PEER, peerId).putExtra(EXTRA_TOKEN, inAppToken)
            return i
        }

        /** Shared URIs we may read: other apps' `content:` only (no `file:`, none of ours). */
        private fun incomingUris(ctx: Context, intent: Intent): List<Uri> = when (intent.action) {
            Intent.ACTION_SEND -> listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
            Intent.ACTION_SEND_MULTIPLE ->
                IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            else -> emptyList()
        }.filter { UriSource.isReadable(ctx, it) }
    }
}
