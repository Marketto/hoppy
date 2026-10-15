package app.hoppy.ui

import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.os.Bundle
import androidx.core.view.isVisible
import app.hoppy.R
import app.hoppy.core.PeerKey
import app.hoppy.core.QrPayload
import app.hoppy.core.Trust
import app.hoppy.databinding.ActivityQrBinding
import app.hoppy.security.Security
import app.hoppy.service.ReceiveService
import com.google.zxing.BarcodeFormat
import com.journeyapps.barcodescanner.BarcodeEncoder
import kotlin.concurrent.thread

/**
 * QR verification from the overflow menu (spec §7.7), not exported:
 * - [MODE_SHOW] "Show my QR": for another phone/tablet to scan (phone ↔ phone). The one-time
 *   code is accepted by this phone's receiver while the screen is open (at most 10 min), so
 *   receiving is switched on meanwhile; a successful scan shows ✓ and closes.
 * - [MODE_SCAN] "Scan a QR": a desktop's (or phone's) code; verified here, then the code goes
 *   to that device over a session that must present exactly the scanned key.
 */
class QrActivity : ThemedActivity() {
    private lateinit var b: ActivityQrBinding
    private var showing = false
    private var startedReceiver = false
    private val expire = Runnable { onExpired() }

    private val verifiedListener: (PeerKey) -> Unit = { peer ->
        runOnUiThread { if (showing) onScannedByOther(peer) }
    }

    private val scanner = QrScanner(
        this,
        onResult = { payload -> QrScanner.verify(this, payload, null) { _, message -> showStatus(message) } },
        onUnavailable = { showStatus(getString(R.string.verify_no_camera)) },
    )

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        b = ActivityQrBinding.inflate(layoutInflater)
        setContentView(b.root)
        b.close.setOnClickListener { finish() }
        if (intent.getStringExtra(EXTRA_MODE) == MODE_SCAN) {
            b.title.setText(R.string.scan_qr)
            b.hint.setText(R.string.verify_scan_prompt)
            b.qr.isVisible = false
            if (savedInstanceState == null) scanner.start()
            return
        }
        showing = true
        startedReceiver = savedInstanceState?.getBoolean(STATE_STARTED_RECEIVER) ?: false
        if (!ReceiveService.running) {
            ReceiveService.start(this) // the scanner's session must reach this phone
            startedReceiver = true
        }
        Security.addVerifiedListener(verifiedListener)
        val app = applicationContext
        val size = (240 * resources.displayMetrics.density).toInt()
        thread(name = "hoppy-qr") {
            val local = Security.local(app)
            val payload = local.beginQr()
            val bitmap: Bitmap? = runCatching { BarcodeEncoder().encodeBitmap(payload, BarcodeFormat.QR_CODE, size, size) }.getOrNull()
            val fp = QrPayload.parse(payload)?.fingerprint.orEmpty().chunked(4).joinToString(" ")
            runOnUiThread {
                if (isFinishing || isDestroyed) return@runOnUiThread
                b.qr.setImageBitmap(bitmap)
                b.qr.isVisible = bitmap != null
                b.fingerprint.text = getString(R.string.my_fingerprint, fp)
            }
        }
        b.root.postDelayed(expire, Trust.QR_CODE_TTL_MS)
    }

    private fun showStatus(message: String) {
        b.status.text = message
        b.status.isVisible = true
    }

    private fun onScannedByOther(peer: PeerKey) {
        showing = false
        b.qr.isVisible = false
        b.hint.isVisible = false
        showStatus(getString(R.string.verified_by_qr, SafeText.label(peer.name)))
        b.root.postDelayed({ if (!isFinishing) finish() }, 1800)
    }

    private fun onExpired() {
        if (!showing) return
        showing = false
        b.qr.isVisible = false
        showStatus(getString(R.string.my_qr_expired))
        endQr()
    }

    private fun endQr() {
        val app = applicationContext
        thread(name = "hoppy-qr-end") { runCatching { Security.trust(app).endQr() } }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        outState.putBoolean(STATE_STARTED_RECEIVER, startedReceiver)
    }

    override fun onDestroy() {
        if (::b.isInitialized) b.root.removeCallbacks(expire)
        Security.removeVerifiedListener(verifiedListener)
        // Not on a rotation: the recreated screen shows a fresh code meanwhile.
        if (isFinishing && intent.getStringExtra(EXTRA_MODE) != MODE_SCAN) {
            endQr()
            // Receiving was switched on only for the QR code: switch it off again.
            if (startedReceiver) ReceiveService.stop(this)
        }
        super.onDestroy()
    }

    companion object {
        private const val EXTRA_MODE = "app.hoppy.QR_MODE"
        private const val MODE_SHOW = "show"
        private const val MODE_SCAN = "scan"
        private const val STATE_STARTED_RECEIVER = "started_receiver"

        fun showIntent(ctx: Context): Intent = Intent(ctx, QrActivity::class.java).putExtra(EXTRA_MODE, MODE_SHOW)

        fun scanIntent(ctx: Context): Intent = Intent(ctx, QrActivity::class.java).putExtra(EXTRA_MODE, MODE_SCAN)
    }
}
