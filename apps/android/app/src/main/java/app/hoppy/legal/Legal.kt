package app.hoppy.legal

import android.content.Context
import app.hoppy.Prefs
import org.json.JSONObject
import java.util.Locale

/**
 * Bundled legal texts (`assets/legal/legal.json`, written by tools/legal.py from docs/legal/)
 * and the acceptance gate: nothing networked (discovery, receiver, sending, update checks)
 * starts before [accepted] is true. Contract: docs/legal/README.md.
 */
object Legal {
    class Bundle(
        val version: LegalVersion,
        /** language → key → text (keys: title, body, accept_terms, …, see docs/legal/README.md). */
        val strings: Map<String, Map<String, String>>,
        val pages: Map<String, Map<String, String>>,
    )

    @Volatile private var cached: Bundle? = null

    /** Parsed once per process (a ~4 KB asset; the bundled version only changes with an app update). */
    fun bundle(ctx: Context): Bundle {
        cached?.let { return it }
        val text = ctx.applicationContext.assets.open("legal/legal.json").bufferedReader().use { it.readText() }
        val root = JSONObject(text)
        val v = root.getJSONObject("version")
        fun nested(obj: JSONObject): Map<String, Map<String, String>> =
            obj.keys().asSequence().associateWith { k ->
                val inner = obj.getJSONObject(k)
                inner.keys().asSequence().associateWith { inner.getString(it) }
            }
        return Bundle(
            LegalVersion(v.getInt("terms"), v.getInt("privacy")),
            nested(root.getJSONObject("strings")),
            nested(root.getJSONObject("pages")),
        ).also { cached = it }
    }

    /** "it" or "en", from the current locale. */
    fun language(): String = LegalLogic.language(Locale.getDefault().language)

    /** Text of [key] in the user's language (English as fallback). */
    fun text(ctx: Context, key: String): String {
        val s = bundle(ctx).strings
        return s[language()]?.get(key) ?: s["en"]?.get(key).orEmpty()
    }

    /** True when the user accepted at least the bundled terms + privacy versions. */
    fun accepted(ctx: Context): Boolean =
        !LegalLogic.needsAcceptance(bundle(ctx).version, Prefs.legalAccepted(ctx))

    /** First acceptance ever (vs. an updated version): picks title/body vs updated_*. */
    fun firstTime(ctx: Context): Boolean = Prefs.legalAccepted(ctx) == null

    fun accept(ctx: Context) {
        Prefs.setLegalAccepted(ctx, bundle(ctx).version, System.currentTimeMillis())
    }

    /** `file:///android_asset/legal/<page>.<lang>.html`, or null for an unknown page key. */
    fun pageUrl(ctx: Context, page: String): String? =
        LegalLogic.pageFile(bundle(ctx).pages, page, language())?.let { LegalLogic.ASSET_BASE + it }
}
