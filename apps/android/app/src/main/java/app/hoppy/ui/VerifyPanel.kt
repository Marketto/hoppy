package app.hoppy.ui

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import androidx.core.view.isVisible
import app.hoppy.Node
import app.hoppy.R
import app.hoppy.core.KeyChangedException
import app.hoppy.core.PeerKey
import app.hoppy.core.QrPayload
import app.hoppy.core.QrVerification
import app.hoppy.core.Sas
import app.hoppy.core.SessionInfo
import app.hoppy.core.TrustState
import app.hoppy.core.UnknownDeviceException
import app.hoppy.core.VerificationFailedException
import app.hoppy.databinding.ViewVerifyBinding
import app.hoppy.security.Security
import app.hoppy.ui.theme.ThemeFonts
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import java.util.Locale
import kotlin.concurrent.thread

/**
 * The verification code of an encrypted session (spec §7.6) inside `view_verify.xml`:
 * 4 emoji (TalkBack reads their names), a caption with the names, "123" for the 8 digits,
 * and the actions "They match" / "Verify with QR" / "Show my QR" / "Later".
 */
class VerifyPanel(private val b: ViewVerifyBinding, private val actions: Actions) {
    interface Actions {
        fun onMatch(peer: PeerKey)
        fun onScan()
        fun onShowMine()
        fun onLater()
    }

    private var digits = false
    private var session: SessionInfo? = null

    init {
        ThemeFonts.applyBodyBold(b.verifyTitle)
        b.verifyToggle.setOnClickListener { digits = !digits; render() }
        b.verifyMatch.setOnClickListener { session?.peer?.let(actions::onMatch) }
        b.verifyScan.setOnClickListener { actions.onScan() }
        b.verifyShowMine.setOnClickListener { actions.onShowMine() }
        b.verifyLater.setOnClickListener { actions.onLater() }
    }

    /** Shows the code of [s] (an encrypted New/Known session). */
    fun show(s: SessionInfo) {
        session = s
        digits = false
        val ctx = b.root.context
        val name = SafeText.label(s.peer?.name ?: "?")
        b.verifyTitle.text = ctx.getString(
            if (s.trust == TrustState.NEW) R.string.verify_title_new else R.string.verify_title_known, name,
        )
        b.verifyWarning.isVisible = s.nameMatchesKnown
        b.verifyDone.isVisible = false
        b.verifyActions.isVisible = true
        b.root.isVisible = true
        render()
    }

    private fun render() {
        val s = session ?: return
        val sas = s.sas ?: return
        val ctx = b.root.context
        val italian = Locale.getDefault().language == "it"
        val words = sas.words(italian).joinToString(" · ")
        b.verifyEmoji.text = sas.emoji.joinToString(" ") { Sas.TABLE[it].emoji }
        b.verifyEmoji.contentDescription = ctx.getString(R.string.verify_code_cd, sas.words(italian).joinToString(", "))
        b.verifyWords.text = words
        b.verifyDigits.text = sas.digits
        // Digit by digit, so screen readers do not read "nine hundred nine".
        b.verifyDigits.contentDescription = ctx.getString(
            R.string.verify_digits_cd, sas.digits.filter { it.isDigit() }.toList().joinToString(" "),
        )
        b.verifyEmoji.isVisible = !digits
        b.verifyWords.isVisible = !digits
        b.verifyDigits.isVisible = digits
        b.verifyToggle.text = ctx.getString(if (digits) R.string.verify_show_emoji else R.string.verify_show_digits)
        b.verifyToggle.contentDescription =
            ctx.getString(if (digits) R.string.verify_show_emoji_cd else R.string.verify_show_digits_cd)
    }

    /** ✓ after "They match" or a QR scan: the actions go away. */
    fun showDone(message: String) {
        b.verifyDone.text = message
        b.verifyDone.isVisible = true
        b.verifyActions.isVisible = false
        b.verifyWarning.isVisible = false
    }

    fun hide() {
        b.root.isVisible = false
    }

    val visible: Boolean get() = b.root.isVisible

    companion object {
        /** "They match": marks [peer] verified (off the main thread); [done] gets a message for the user. */
        fun markVerified(activity: Activity, peer: PeerKey, done: (ok: Boolean, message: String) -> Unit) {
            val app = activity.applicationContext
            thread(name = "hoppy-verify") {
                val result = try {
                    Security.trust(app).markVerified(peer)
                    true to app.getString(R.string.verify_done, SafeText.label(peer.name))
                } catch (e: KeyChangedException) {
                    false to app.getString(R.string.key_changed_message, SafeText.label(peer.name))
                } catch (e: Exception) {
                    false to app.getString(R.string.verify_save_failed, e.message ?: "?")
                }
                activity.runOnUiThread { done(result.first, result.second) }
            }
        }
    }
}

/**
 * In-app QR scanning (ZXing Android Embedded): asks for the camera permission first; without
 * a camera or permission [onUnavailable] runs (fall back to comparing the code). Create it as
 * a property of the activity (it registers activity-result launchers).
 */
class QrScanner(
    private val activity: ComponentActivity,
    private val onResult: (String) -> Unit,
    private val onUnavailable: () -> Unit,
) {
    private val scan = activity.registerForActivityResult(ScanContract()) { result ->
        result.contents?.let(onResult)
    }
    private val permission = activity.registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) launch() else onUnavailable()
    }

    fun start() {
        if (!activity.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY)) {
            onUnavailable()
            return
        }
        if (ContextCompat.checkSelfPermission(activity, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            launch()
        } else {
            permission.launch(Manifest.permission.CAMERA)
        }
    }

    private fun launch() {
        val options = ScanOptions()
        options.setDesiredBarcodeFormats(ScanOptions.QR_CODE)
        options.setPrompt(activity.getString(R.string.verify_scan_prompt))
        options.setBeepEnabled(false)
        options.setOrientationLocked(false)
        scan.launch(options)
    }

    companion object {
        private const val PEER_WAIT_MS = 3_000L

        /**
         * Runs the scanner flow of spec §7.7 on a background thread ([live]: the session on
         * screen, if any) and reports a user message. Never sends the code to another key.
         */
        fun verify(activity: Activity, payload: String, live: PeerKey?, done: (ok: Boolean, message: String) -> Unit) {
            val app: Context = activity.applicationContext
            Node.acquire(app) // the shower is reached through discovery (direct or callback)
            thread(name = "hoppy-qr-verify") {
                val result = try {
                    // The node may have just started: give discovery up to ~3 s to find the
                    // device that shows the code (asking everyone to announce) before scanning
                    // with what is known; without it the code cannot be confirmed over there.
                    val wanted = QrPayload.parse(payload)?.id
                    if (wanted != null && Node.peers().none { it.device.id == wanted }) {
                        Node.refresh()
                        val end = System.currentTimeMillis() + PEER_WAIT_MS
                        while (Node.peers().none { it.device.id == wanted } && System.currentTimeMillis() < end) {
                            Thread.sleep(100)
                        }
                    }
                    val r = QrVerification.scan(payload, live, Security.local(app), Node.peers(), Node.node())
                    val name = SafeText.label(r.peer.name)
                    true to app.getString(if (r.confirmed) R.string.verify_qr_ok else R.string.verify_qr_ok_local, name)
                } catch (e: IllegalArgumentException) {
                    false to app.getString(R.string.verify_qr_not_hoppy)
                } catch (e: UnknownDeviceException) {
                    false to app.getString(R.string.verify_qr_unknown)
                } catch (e: VerificationFailedException) {
                    false to app.getString(R.string.verify_qr_mismatch)
                } catch (e: KeyChangedException) {
                    false to app.getString(R.string.key_changed_message, SafeText.label(e.peer.name))
                } catch (e: Exception) {
                    false to app.getString(R.string.verify_failed, e.message ?: "?")
                } finally {
                    Node.release()
                }
                activity.runOnUiThread { done(result.first, result.second) }
            }
        }
    }
}
