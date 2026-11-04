package app.hoppy.ui

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.graphics.Paint
import android.net.Uri
import android.os.Bundle
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Button
import android.widget.Toast
import androidx.activity.addCallback
import app.hoppy.R
import app.hoppy.databinding.ActivityLegalPageBinding
import app.hoppy.legal.Legal
import app.hoppy.legal.LegalLogic
import app.hoppy.ui.theme.ThemeFonts
import java.io.ByteArrayInputStream

/**
 * One bundled legal page (terms / privacy / licences, `assets/legal/`, tools/legal.py) in a
 * locked-down WebView: no JavaScript, no file/content access beyond the assets, no network
 * loads. Links between the bundled pages stay here; `https://` links open in the browser;
 * anything else is ignored. Not exported; the intent carries a page *key*, never a URL.
 */
class LegalPageActivity : ThemedActivity() {
    private lateinit var b: ActivityLegalPageBinding

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        b = ActivityLegalPageBinding.inflate(layoutInflater)
        setContentView(b.root)
        val page = intent.getStringExtra(EXTRA_PAGE)?.takeIf { it in LegalLogic.PAGES } ?: "terms"
        b.title.text = Legal.text(this, page)
        ThemeFonts.applyDisplay(b.title)
        b.close.setOnClickListener { finish() }

        val web = b.web
        with(web.settings) {
            javaScriptEnabled = false
            allowFileAccess = false // file:///android_asset stays readable (assets are exempt)
            allowContentAccess = false
            @Suppress("DEPRECATION")
            allowFileAccessFromFileURLs = false
            @Suppress("DEPRECATION")
            allowUniversalAccessFromFileURLs = false
            blockNetworkImage = true
            blockNetworkLoads = true
            domStorageEnabled = false
            setGeolocationEnabled(false)
            setSupportMultipleWindows(false)
            javaScriptCanOpenWindowsAutomatically = false
        }
        web.webViewClient = object : WebViewClient() {
            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                val url = request.url?.toString()
                return when {
                    LegalLogic.isBundledPage(url) -> false
                    LegalLogic.isExternal(url) -> { openExternal(url!!); true }
                    else -> true
                }
            }

            // Belt and braces: only our pages are ever loaded, whatever a page references.
            override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? =
                if (LegalLogic.isBundledPage(request.url?.toString())) {
                    null
                } else {
                    WebResourceResponse("text/plain", "utf-8", 403, "Forbidden", emptyMap(), ByteArrayInputStream(ByteArray(0)))
                }
        }
        onBackPressedDispatcher.addCallback(this) {
            if (web.canGoBack()) web.goBack() else finish()
        }
        val restored = savedInstanceState?.let { web.restoreState(it) } != null
        if (!restored) Legal.pageUrl(this, page)?.let { web.loadUrl(it) }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        b.web.saveState(outState)
    }

    override fun onDestroy() {
        if (::b.isInitialized) b.web.destroy()
        super.onDestroy()
    }

    private fun openExternal(url: String) {
        val i = Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE)
        try {
            startActivity(i)
        } catch (_: ActivityNotFoundException) {
            Toast.makeText(this, R.string.legal_no_browser, Toast.LENGTH_LONG).show()
        }
    }

    companion object {
        private const val EXTRA_PAGE = "app.hoppy.LEGAL_PAGE"

        fun intent(ctx: Context, page: String): Intent =
            Intent(ctx, LegalPageActivity::class.java).putExtra(EXTRA_PAGE, page)
    }
}

/** The small "Terms · Privacy · Licences" links (About dialog, first-run modal). */
object LegalLinks {
    fun bind(activity: Activity, terms: Button?, privacy: Button?, licenses: Button?) {
        listOf("terms" to terms, "privacy" to privacy, "licenses" to licenses).forEach { (page, button) ->
            button ?: return@forEach
            val label = Legal.text(activity, page)
            button.text = label
            button.paintFlags = button.paintFlags or Paint.UNDERLINE_TEXT_FLAG
            button.contentDescription = activity.getString(R.string.legal_link_cd, label)
            button.setOnClickListener { activity.startActivity(LegalPageActivity.intent(activity, page)) }
        }
    }
}
