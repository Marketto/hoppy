package app.hoppy.ui

import android.content.Context
import android.content.Intent
import android.content.res.ColorStateList
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.InsetDrawable
import android.os.Bundle
import android.util.TypedValue
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.LinearLayout
import androidx.activity.addCallback
import androidx.annotation.AttrRes
import androidx.appcompat.view.ContextThemeWrapper
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.PagerSnapHelper
import androidx.recyclerview.widget.RecyclerView
import app.hoppy.Prefs
import app.hoppy.R
import app.hoppy.databinding.ActivityThemePickerBinding
import app.hoppy.databinding.ItemThemeCardBinding
import app.hoppy.ui.theme.HoppyTheme
import app.hoppy.ui.theme.Suggestion
import app.hoppy.ui.theme.SuggestionReason
import app.hoppy.ui.theme.SystemThemeSignals
import app.hoppy.ui.theme.ThemeFonts
import app.hoppy.ui.theme.dp

/**
 * "Choose your Hoppy": the first-run (installation-time) theme choice, also opened
 * from the overflow menu. A horizontal pager of live previews, each inflated in its
 * own theme; the OS-based suggestion is pre-selected and marked "Recommended".
 */
class ThemePickerActivity : ThemedActivity() {
    private lateinit var b: ActivityThemePickerBinding
    private lateinit var suggestion: Suggestion
    private val themes = HoppyTheme.entries
    private var selected = 0
    private var firstRun = false

    /** On first run the picker itself wears the suggested theme. */
    override fun wantedTheme(): HoppyTheme =
        if (intent.getBooleanExtra(EXTRA_FIRST_RUN, false)) SystemThemeSignals.suggestion(this).theme else Prefs.theme(this)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        b = ActivityThemePickerBinding.inflate(layoutInflater)
        setContentView(b.root)
        firstRun = intent.getBooleanExtra(EXTRA_FIRST_RUN, false)
        suggestion = SystemThemeSignals.suggestion(this)
        val initial = if (firstRun) suggestion.theme else Prefs.theme(this)
        selected = savedInstanceState?.getInt(STATE_SELECTED, initial.ordinal) ?: initial.ordinal

        ThemeFonts.applyDisplay(b.title, b.titleDecor, b.use)

        val lm = b.cards.layoutManager as LinearLayoutManager
        b.cards.adapter = CardAdapter()
        b.cards.setHasFixedSize(true)
        val snap = PagerSnapHelper()
        snap.attachToRecyclerView(b.cards)
        lm.scrollToPositionWithOffset(selected, 0)
        b.cards.addOnScrollListener(object : RecyclerView.OnScrollListener() {
            override fun onScrollStateChanged(recyclerView: RecyclerView, newState: Int) {
                if (newState != RecyclerView.SCROLL_STATE_IDLE) return
                val v = snap.findSnapView(lm) ?: return
                val pos = lm.getPosition(v)
                if (pos != RecyclerView.NO_POSITION) select(pos)
            }
        })

        buildDots()
        select(selected)
        b.use.setOnClickListener { confirm(themes[selected]) }
        onBackPressedDispatcher.addCallback(this) {
            // First run: leaving without choosing means "the suggested one is fine".
            if (firstRun) confirm(suggestion.theme) else finish()
        }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        outState.putInt(STATE_SELECTED, selected)
    }

    private fun select(pos: Int) {
        selected = pos
        for (i in 0 until b.dots.childCount) {
            val dot = b.dots.getChildAt(i)
            val on = i == pos
            dot.backgroundTintList = ColorStateList.valueOf(
                themeColor(if (on) androidx.appcompat.R.attr.colorPrimary else com.google.android.material.R.attr.colorOutline),
            )
            dot.scaleX = if (on) 1.35f else 1f
            dot.scaleY = if (on) 1.35f else 1f
        }
        b.use.contentDescription = getString(R.string.theme_use) + ": " + getString(themes[pos].label)
    }

    private fun buildDots() {
        val size = dp(8f).toInt()
        val gap = dp(5f).toInt()
        themes.forEach { _ ->
            b.dots.addView(
                View(this).apply {
                    setBackgroundResource(R.drawable.dot_indicator)
                    importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO
                },
                LinearLayout.LayoutParams(size, size).apply { setMargins(gap, 0, gap, 0) },
            )
        }
    }

    private fun confirm(theme: HoppyTheme) {
        Prefs.setTheme(this, theme)
        Prefs.setThemeChosen(this, true)
        if (firstRun) {
            startActivity(Intent(this, MainActivity::class.java))
        }
        finish()
    }

    private fun themeColor(@AttrRes attr: Int): Int {
        val tv = TypedValue()
        theme.resolveAttribute(attr, tv, true)
        return tv.data
    }

    private inner class CardAdapter : RecyclerView.Adapter<CardAdapter.Holder>() {
        inner class Holder(val b: ItemThemeCardBinding) : RecyclerView.ViewHolder(b.root)

        override fun getItemCount() = themes.size

        // One view type per theme: each card is inflated in its own theme.
        override fun getItemViewType(position: Int) = position

        override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): Holder {
            val theme = themes[viewType]
            val themed = ContextThemeWrapper(parent.context, theme.styleRes)
            val cb = ItemThemeCardBinding.inflate(LayoutInflater.from(themed), parent, false)
            styleFrame(themed, cb)
            ThemeFonts.applyDisplay(cb.name, cb.tagline, cb.pill)
            cb.name.setText(theme.label)
            cb.description.setText(theme.description)
            cb.tagline.setText(theme.tagline)
            val isSuggested = theme == suggestion.theme
            cb.pill.visibility = if (isSuggested) View.VISIBLE else View.INVISIBLE
            cb.pill.text = when (suggestion.reason) {
                SuggestionReason.DEFAULT -> getString(R.string.theme_recommended)
                SuggestionReason.DARK_MODE -> getString(R.string.theme_recommended_reason, getString(R.string.theme_reason_dark))
                SuggestionReason.HIGH_CONTRAST ->
                    getString(R.string.theme_recommended_reason, getString(R.string.theme_reason_contrast))
            }
            val extra = if (isSuggested) cb.pill.text else ""
            cb.root.contentDescription = getString(
                R.string.theme_card_description, getString(theme.label), viewType + 1, themes.size,
                listOf(getString(theme.description), extra).filter { it.isNotEmpty() }.joinToString(" "),
            )
            cb.root.setOnClickListener {
                select(viewType)
                b.cards.smoothScrollToPosition(viewType)
                it.announceForAccessibility(getString(R.string.theme_selected, getString(theme.label)))
            }
            return Holder(cb)
        }

        // Everything is bound once in onCreateViewHolder (one view type per theme).
        override fun onBindViewHolder(holder: Holder, position: Int) = Unit

        /** Corner radius, frame stroke and (Showa/Manga) hard offset shadow of the card. */
        private fun styleFrame(ctx: Context, cb: ItemThemeCardBinding) {
            val dm = ctx.resources.displayMetrics
            fun value(@AttrRes attr: Int) = TypedValue().also { ctx.theme.resolveAttribute(attr, it, true) }
            val corner = value(R.attr.hoppyCorner).getDimension(dm)
            val stroke = value(R.attr.hoppyFrameStroke).getDimension(dm).toInt()
            val strokeColor = value(R.attr.hoppyFrameStrokeColor).data
            val shadow = value(R.attr.hoppyCardShadow).getDimension(dm).toInt()
            val ink = value(R.attr.hoppyInk).data
            val bg = value(android.R.attr.colorBackground).data

            cb.frame.background = GradientDrawable().apply {
                cornerRadius = corner
                setColor(bg)
            }
            cb.frame.clipToOutline = true
            cb.frame.foreground = GradientDrawable().apply {
                cornerRadius = corner
                setStroke(stroke, strokeColor)
            }
            val lp = cb.frame.layoutParams as ViewGroup.MarginLayoutParams
            lp.marginEnd = shadow
            lp.bottomMargin = shadow
            cb.frame.layoutParams = lp
            cb.root.background = if (shadow > 0) {
                InsetDrawable(GradientDrawable().apply { cornerRadius = corner; setColor(ink) }, shadow, shadow, 0, 0)
            } else {
                null
            }
        }
    }

    companion object {
        private const val EXTRA_FIRST_RUN = "app.hoppy.FIRST_RUN"
        private const val STATE_SELECTED = "selected"

        fun intent(ctx: Context, firstRun: Boolean) =
            Intent(ctx, ThemePickerActivity::class.java).putExtra(EXTRA_FIRST_RUN, firstRun)
    }
}
