package app.hoppy.ui

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.os.Bundle
import androidx.activity.addCallback
import androidx.core.view.isVisible
import app.hoppy.databinding.ActivityLegalBinding
import app.hoppy.legal.Legal
import app.hoppy.ui.theme.ThemeFonts

/**
 * First-run (and updated-terms) acceptance, before the theme picker, home and the share
 * screen (docs/legal/README.md): mascot, title + body from `assets/legal/legal.json`, links to
 * the Terms and the Privacy policy, three checkboxes (terms, arts. 1341–1342 c.c. clauses,
 * privacy read). *Continue* stays disabled until all three are ticked; *Quit* leaves without
 * doing anything. Nothing networked starts before acceptance (`Node.acquire`, `ReceiveService`).
 *
 * Two modes: [gateIntent] (from home, reopens home after acceptance; *Quit* closes our task)
 * and [resultIntent] (from `SendActivity`, answers RESULT_OK / RESULT_CANCELED so the share
 * screen keeps its URI grants and decides itself).
 */
class LegalActivity : ThemedActivity() {
    private lateinit var b: ActivityLegalBinding
    private val forResult get() = intent.getBooleanExtra(EXTRA_FOR_RESULT, false)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        b = ActivityLegalBinding.inflate(layoutInflater)
        setContentView(b.root)
        val first = Legal.firstTime(this)
        b.title.text = Legal.text(this, if (first) "title" else "updated_title")
        b.body.text = Legal.text(this, if (first) "body" else "updated_body")
        b.acceptTerms.text = Legal.text(this, "accept_terms")
        b.approveClauses.text = Legal.text(this, "approve_clauses")
        b.readPrivacy.text = Legal.text(this, "read_privacy")
        b.accept.text = Legal.text(this, "continue")
        b.quit.text = Legal.text(this, "quit")
        ThemeFonts.applyDisplay(b.title, b.accept)
        LegalLinks.bind(this, b.linkTerms, b.linkPrivacy, b.linkLicenses)

        listOf(b.acceptTerms, b.approveClauses, b.readPrivacy).forEach { box ->
            box.setOnCheckedChangeListener { _, _ -> sync() }
        }
        sync()
        b.accept.setOnClickListener { accept() }
        b.quit.setOnClickListener { quit() }
        onBackPressedDispatcher.addCallback(this) { quit() }
    }

    private fun allTicked() = b.acceptTerms.isChecked && b.approveClauses.isChecked && b.readPrivacy.isChecked

    private fun sync() {
        val ok = allTicked()
        b.accept.isEnabled = ok
        b.hint.isVisible = !ok
    }

    private fun accept() {
        if (!allTicked()) return
        Legal.accept(this)
        if (forResult) {
            setResult(Activity.RESULT_OK)
        } else {
            // Home decides what comes next (theme picker on first run).
            startActivity(Intent(this, MainActivity::class.java))
        }
        finish()
    }

    private fun quit() {
        if (forResult) {
            // The share screen finishes itself: never remove a task that may be another app's.
            setResult(Activity.RESULT_CANCELED)
            finish()
        } else {
            finishAndRemoveTask()
        }
    }

    companion object {
        private const val EXTRA_FOR_RESULT = "app.hoppy.LEGAL_FOR_RESULT"

        /** From home: on acceptance home is opened again. */
        fun gateIntent(ctx: Context): Intent = Intent(ctx, LegalActivity::class.java)

        /** From the share screen (registerForActivityResult). */
        fun resultIntent(ctx: Context): Intent =
            Intent(ctx, LegalActivity::class.java).putExtra(EXTRA_FOR_RESULT, true)
    }
}
