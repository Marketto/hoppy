package app.hoppy.ui.theme

import android.app.UiModeManager
import android.content.Context
import android.content.res.Configuration
import android.os.Build
import android.provider.Settings
import androidx.annotation.RequiresApi

/** Why a theme was suggested; the picker shows it next to the "Recommended" pill. */
enum class SuggestionReason { DEFAULT, DARK_MODE, HIGH_CONTRAST }

data class Suggestion(val theme: HoppyTheme, val reason: SuggestionReason)

/**
 * First-run theme suggestion from OS accessibility/appearance settings.
 * Pure logic (no Android calls) so it is unit-tested on the JVM.
 */
object ThemeSuggestion {
    fun suggest(highContrast: Boolean, darkMode: Boolean): HoppyTheme =
        suggestWithReason(highContrast, darkMode).theme

    fun suggestWithReason(highContrast: Boolean, darkMode: Boolean): Suggestion = when {
        highContrast -> Suggestion(HoppyTheme.MANGA, SuggestionReason.HIGH_CONTRAST)
        darkMode -> Suggestion(HoppyTheme.NIGHT, SuggestionReason.DARK_MODE)
        else -> Suggestion(HoppyTheme.PURIKURA, SuggestionReason.DEFAULT)
    }
}

/** Reads the OS signals used by [ThemeSuggestion]. */
object SystemThemeSignals {
    fun highContrast(ctx: Context): Boolean {
        val contrast = if (Build.VERSION.SDK_INT >= 34) systemContrast(ctx) else 0f
        // Hidden but long-standing secure setting behind "High contrast text".
        val highText = runCatching {
            Settings.Secure.getInt(ctx.contentResolver, "high_text_contrast_enabled", 0) == 1
        }.getOrDefault(false)
        return contrast >= 0.5f || highText
    }

    /** Android 14+ system contrast level (-1..1; 0.5 = medium, 1 = high). */
    @RequiresApi(34)
    private fun systemContrast(ctx: Context): Float =
        runCatching { ctx.getSystemService(UiModeManager::class.java)?.contrast ?: 0f }.getOrDefault(0f)

    fun darkMode(ctx: Context): Boolean =
        (ctx.resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES

    fun suggestion(ctx: Context): Suggestion =
        ThemeSuggestion.suggestWithReason(highContrast(ctx), darkMode(ctx))
}
