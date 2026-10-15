package app.hoppy.ui

import android.app.Activity
import androidx.core.view.isVisible
import app.hoppy.R
import app.hoppy.core.KeyChangedException
import app.hoppy.core.PeerKey
import app.hoppy.core.Sas
import app.hoppy.core.SessionInfo
import app.hoppy.core.TrustState
import app.hoppy.databinding.ViewVerifyBinding
import app.hoppy.security.Security
import app.hoppy.ui.theme.ThemeFonts
import java.util.Locale
import kotlin.concurrent.thread

/**
 * The verification code of an encrypted session (spec §7.6) inside `view_verify.xml`:
 * 4 large emoji (TalkBack reads their names), a caption with the names, and the actions
 * "They match" / "Later". Emoji are the only verification method (no digits, no QR).
 */
class VerifyPanel(private val b: ViewVerifyBinding, private val actions: Actions) {
    interface Actions {
        fun onMatch(peer: PeerKey)
        fun onLater()
    }

    private var session: SessionInfo? = null

    init {
        ThemeFonts.applyBodyBold(b.verifyTitle)
        b.verifyMatch.setOnClickListener { session?.peer?.let(actions::onMatch) }
        b.verifyLater.setOnClickListener { actions.onLater() }
    }

    /** Shows the code of [s] (an encrypted New/Known session). */
    fun show(s: SessionInfo) {
        session = s
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
    }

    /** ✓ after "They match": the actions go away. */
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

