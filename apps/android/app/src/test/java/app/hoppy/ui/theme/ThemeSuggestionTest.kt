package app.hoppy.ui.theme

import org.junit.Test
import kotlin.test.assertEquals

class ThemeSuggestionTest {
    @Test
    fun defaultIsPurikura() {
        assertEquals(HoppyTheme.PURIKURA, ThemeSuggestion.suggest(highContrast = false, darkMode = false))
        assertEquals(SuggestionReason.DEFAULT, ThemeSuggestion.suggestWithReason(false, false).reason)
    }

    @Test
    fun darkModeSuggestsNight() {
        assertEquals(HoppyTheme.NIGHT, ThemeSuggestion.suggest(highContrast = false, darkMode = true))
        assertEquals(SuggestionReason.DARK_MODE, ThemeSuggestion.suggestWithReason(false, true).reason)
    }

    @Test
    fun highContrastSuggestsManga() {
        assertEquals(HoppyTheme.MANGA, ThemeSuggestion.suggest(highContrast = true, darkMode = false))
        assertEquals(SuggestionReason.HIGH_CONTRAST, ThemeSuggestion.suggestWithReason(true, false).reason)
    }

    @Test
    fun highContrastWinsOverDarkMode() {
        assertEquals(HoppyTheme.MANGA, ThemeSuggestion.suggest(highContrast = true, darkMode = true))
        assertEquals(SuggestionReason.HIGH_CONTRAST, ThemeSuggestion.suggestWithReason(true, true).reason)
    }

    @Test
    fun storedValuesMigrate() {
        assertEquals(HoppyTheme.PURIKURA, HoppyTheme.fromStored("KAWAII"))
        assertEquals(HoppyTheme.NIGHT, HoppyTheme.fromStored("PRO"))
        assertEquals(HoppyTheme.YUME, HoppyTheme.fromStored("YUME"))
        assertEquals(null, HoppyTheme.fromStored("NOPE"))
        assertEquals(null, HoppyTheme.fromStored(null))
    }
}
