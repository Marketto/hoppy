package app.hoppy.legal

/**
 * Pure rules of the first-run acceptance (docs/legal/README.md "For app integrators"), kept
 * free of `android.*` so they are unit-tested off-device.
 */
data class LegalVersion(val terms: Int, val privacy: Int)

object LegalLogic {
    /** Page keys of `legal.json` "pages"; nothing else is ever opened. */
    val PAGES = listOf("terms", "privacy", "licenses")

    /** Directory of the bundled pages, as a WebView URL. */
    const val ASSET_BASE = "file:///android_asset/legal/"

    private val pageFile = Regex("""(terms|privacy|licenses)\.(it|en)\.html""")

    /**
     * The modal is due when nothing was accepted yet or when either bundled number is higher
     * than the stored one (a lower bundled number, e.g. after a downgrade, does not ask again).
     */
    fun needsAcceptance(bundled: LegalVersion, stored: LegalVersion?): Boolean =
        stored == null || bundled.terms > stored.terms || bundled.privacy > stored.privacy

    /** Italian when the system language is Italian, else English (the two shipped languages). */
    fun language(languageTag: String?): String =
        if (languageTag?.lowercase()?.substringBefore('-')?.substringBefore('_') == "it") "it" else "en"

    /**
     * True for a bundled legal page (`file:///android_asset/legal/terms.it.html`, optionally
     * with a `#fragment`); anything else under file:, any other asset and any path trick is
     * refused.
     */
    fun isBundledPage(url: String?): Boolean {
        if (url == null || !url.startsWith(ASSET_BASE)) return false
        val rest = url.removePrefix(ASSET_BASE).substringBefore('#')
        return pageFile.matches(rest)
    }

    /** Links a page may hand to the system browser: plain https only. */
    fun isExternal(url: String?): Boolean =
        url != null && url.length <= 2048 && url.lowercase().startsWith("https://") &&
            url.none { it.isWhitespace() || it.code < 0x20 }

    /** File name of [page] in [lang] from the `pages` map, only if it is one of ours. */
    fun pageFile(pages: Map<String, Map<String, String>>, page: String, lang: String): String? {
        if (page !in PAGES) return null
        val name = pages[page]?.get(lang) ?: pages[page]?.get("en") ?: return null
        return name.takeIf { pageFile.matches(it) }
    }
}
