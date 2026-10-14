package app.hoppy.ui.theme

import android.content.Context
import android.util.TypedValue
import androidx.annotation.DrawableRes
import androidx.annotation.StringRes
import androidx.annotation.StyleRes
import app.hoppy.R

/**
 * The five Hoppy looks (docs/design/themes.md). The order is the order of the theme
 * picker and must match the `hoppyThemeKind` enum values in res/values/attrs.xml.
 */
enum class HoppyTheme(
    @StyleRes val styleRes: Int,
    /** Idle mascot: AnimatedVectorDrawable (ears, blink, blush / glint). */
    @DrawableRes val mascotAnim: Int,
    /** Mascot shown by `MascotView.celebrate()`. */
    @DrawableRes val happyAnim: Int,
    @StringRes val label: Int,
    @StringRes val description: Int,
    /** Japanese decor line shown on the theme picker card (not translated). */
    @StringRes val tagline: Int,
    val isDark: Boolean,
) {
    PURIKURA(
        R.style.Theme_Hoppy_Purikura, R.drawable.mascot_kawaii_anim, R.drawable.mascot_happy_anim,
        R.string.theme_purikura, R.string.theme_desc_purikura, R.string.decor_tagline_purikura, false,
    ),
    SHOWA(
        R.style.Theme_Hoppy_Showa, R.drawable.mascot_kawaii_anim, R.drawable.mascot_happy_anim,
        R.string.theme_showa, R.string.theme_desc_showa, R.string.decor_tagline_showa, false,
    ),
    MANGA(
        R.style.Theme_Hoppy_Manga, R.drawable.mascot_kawaii_anim, R.drawable.mascot_happy_anim,
        R.string.theme_manga, R.string.theme_desc_manga, R.string.decor_pyon_loud, false,
    ),
    YUME(
        R.style.Theme_Hoppy_Yume, R.drawable.mascot_kawaii_anim, R.drawable.mascot_happy_anim,
        R.string.theme_yume, R.string.theme_desc_yume, R.string.decor_tagline_yume, false,
    ),
    NIGHT(
        R.style.Theme_Hoppy_Night, R.drawable.mascot_pro_anim, R.drawable.mascot_pro_anim,
        R.string.theme_night, R.string.theme_desc_night, R.string.decor_good_evening, true,
    );

    companion object {
        val DEFAULT = PURIKURA

        /** Parses a stored name, migrating the pre-v2 values (KAWAII / PRO). */
        fun fromStored(value: String?): HoppyTheme? = when (value) {
            null -> null
            "KAWAII" -> PURIKURA
            "PRO" -> NIGHT
            else -> entries.firstOrNull { it.name == value }
        }

        /** The theme a (possibly ContextThemeWrapper-ed) context is styled with. */
        fun of(ctx: Context): HoppyTheme {
            val tv = TypedValue()
            return if (ctx.theme.resolveAttribute(R.attr.hoppyThemeKind, tv, true)) {
                entries.getOrElse(tv.data) { DEFAULT }
            } else {
                DEFAULT
            }
        }
    }
}
