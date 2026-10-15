package app.hoppy.ui

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.widget.Toast
import androidx.core.view.isVisible
import app.hoppy.R
import app.hoppy.core.PeerKey
import app.hoppy.databinding.ActivityVerifyBinding
import app.hoppy.security.PendingVerifications
import app.hoppy.security.Security
import kotlin.concurrent.thread

/**
 * Receiver side of device verification (spec §7.5–7.7), opened only from our own
 * notifications (not exported):
 * - [EXTRA_PENDING]: the "Verify" action of an incoming offer: its code (emoji / 123), "They
 *   match", "Verify with QR", "Show my QR", "Later". Accept/Decline stay in the notification
 *   (declined by default after 60 s).
 * - [EXTRA_FORGET_ID]: a sender with a different key was refused: explain, offer
 *   "Forget device" (the next contact is New).
 */
class VerifyActivity : ThemedActivity() {
    private lateinit var b: ActivityVerifyBinding
    private var panel: VerifyPanel? = null
    private var live: PeerKey? = null
    private var pendingId = -1

    private val scanner = QrScanner(
        this,
        onResult = { payload ->
            QrScanner.verify(this, payload, live) { ok, message -> onVerifyResult(ok, message) }
        },
        onUnavailable = { Toast.makeText(this, R.string.verify_no_camera, Toast.LENGTH_LONG).show() },
    )

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        b = ActivityVerifyBinding.inflate(layoutInflater)
        setContentView(b.root)
        b.close.setOnClickListener { finish() }
        val forgetId = intent.getStringExtra(EXTRA_FORGET_ID)
        if (forgetId != null) {
            showKeyChanged(forgetId, SafeText.label(intent.getStringExtra(EXTRA_NAME) ?: "?"))
            return
        }
        pendingId = intent.getIntExtra(EXTRA_PENDING, -1)
        val session = PendingVerifications.get(pendingId)?.session
        if (session == null || session.peer == null || session.sas == null) {
            finish() // the offer was answered or timed out meanwhile
            return
        }
        live = session.peer
        panel = VerifyPanel(
            b.verify,
            object : VerifyPanel.Actions {
                override fun onMatch(peer: PeerKey) {
                    VerifyPanel.markVerified(this@VerifyActivity, peer) { ok, message -> onVerifyResult(ok, message) }
                }
                override fun onScan() = scanner.start()
                override fun onShowMine() = startActivity(QrActivity.showIntent(this@VerifyActivity))
                override fun onLater() = finish()
            },
        ).also { it.show(session) }
    }

    private fun onVerifyResult(ok: Boolean, message: String) {
        if (!ok) {
            Toast.makeText(this, message, Toast.LENGTH_LONG).show()
            return
        }
        PendingVerifications.get(pendingId)?.onVerified?.invoke()
        panel?.showDone(message)
        b.mascot.celebrate()
        b.root.postDelayed({ if (!isFinishing) finish() }, 1500)
    }

    private fun showKeyChanged(id: String, name: String) {
        b.verify.root.isVisible = false
        b.keyChanged.isVisible = true
        b.keyChangedText.text = getString(R.string.key_changed_message_rx, name)
        b.cancel.setOnClickListener { finish() }
        b.forget.setOnClickListener {
            val app = applicationContext
            thread(name = "hoppy-forget") {
                val result = runCatching { Security.trust(app).forget(id) }
                runOnUiThread {
                    val error = result.exceptionOrNull()
                    val message = if (error == null) {
                        getString(R.string.forgotten, name)
                    } else {
                        getString(R.string.forget_failed, name, error.message ?: "?")
                    }
                    Toast.makeText(app, message, Toast.LENGTH_LONG).show()
                    if (error == null) finish()
                }
            }
        }
    }

    companion object {
        private const val EXTRA_PENDING = "app.hoppy.PENDING"
        private const val EXTRA_FORGET_ID = "app.hoppy.FORGET_ID"
        private const val EXTRA_NAME = "app.hoppy.NAME"

        fun pendingIntent(ctx: Context, id: Int): Intent =
            Intent(ctx, VerifyActivity::class.java).putExtra(EXTRA_PENDING, id).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

        fun keyChangedIntent(ctx: Context, id: String, name: String): Intent =
            Intent(ctx, VerifyActivity::class.java)
                .putExtra(EXTRA_FORGET_ID, id)
                .putExtra(EXTRA_NAME, name)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }
}
