//! Desktop themes (docs/design/themes.md): the five looks of the send window, which one is
//! in use, and the operating-system signals that pick one when the user never chose.
//!
//! The rule is Android's `ThemeSuggestion`: **high contrast → Manga**, else **dark mode →
//! Night**, else **Purikura**. A choice from the tray's "Theme" submenu is saved in
//! device.json (`theme`) and always wins; an empty or unknown value means "follow the
//! system". Pure Rust, tested on Linux; the signals are read by `platform::system_look`.

/// One of the five themes, in the order of the tray submenu (same as Android `HoppyTheme`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Theme {
    Purikura,
    Showa,
    Manga,
    Yume,
    Night,
}

impl Theme {
    pub const ALL: [Theme; 5] = [
        Theme::Purikura,
        Theme::Showa,
        Theme::Manga,
        Theme::Yume,
        Theme::Night,
    ];

    /// Stable id: device.json value, CSS `data-theme`, web asset names.
    pub fn id(self) -> &'static str {
        match self {
            Theme::Purikura => "purikura",
            Theme::Showa => "showa",
            Theme::Manga => "manga",
            Theme::Yume => "yume",
            Theme::Night => "night",
        }
    }

    pub fn from_id(id: &str) -> Option<Theme> {
        Theme::ALL.into_iter().find(|t| t.id() == id)
    }

    /// Menu label: the theme names are proper names, the same in Italian and English.
    pub fn label(self) -> &'static str {
        match self {
            Theme::Purikura => "Purikura",
            Theme::Showa => "Showa",
            Theme::Manga => "Manga",
            Theme::Yume => "Yume",
            Theme::Night => "Night",
        }
    }

    pub fn is_dark(self) -> bool {
        self == Theme::Night
    }

    /// Window background before the page paints (no white flash): the theme's `--bg`.
    pub fn background_rgb(self) -> (u8, u8, u8) {
        match self {
            Theme::Purikura => (0xFF, 0xE9, 0xF0),
            Theme::Showa => (0xF4, 0xE9, 0xD0),
            Theme::Manga => (0xFF, 0xFF, 0xFF),
            Theme::Yume => (0xF6, 0xD8, 0xF2),
            Theme::Night => (0x14, 0x15, 0x1B),
        }
    }
}

/// What the operating system says about the user's display preferences.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemLook {
    pub dark: bool,
    pub high_contrast: bool,
}

/// The theme to suggest when the user never chose (Android `ThemeSuggestion`).
pub fn suggest(look: SystemLook) -> Theme {
    if look.high_contrast {
        Theme::Manga
    } else if look.dark {
        Theme::Night
    } else {
        Theme::Purikura
    }
}

/// The theme in use: the saved choice (device.json `theme`), else the suggestion.
pub fn effective(chosen: &str, look: SystemLook) -> Theme {
    Theme::from_id(chosen.trim()).unwrap_or_else(|| suggest(look))
}

/// `gsettings get org.gnome.desktop.interface color-scheme` → `'prefer-dark'` = dark.
/// `None` when the key does not exist (older GNOME, other desktops).
pub fn gsettings_color_scheme_dark(output: &str) -> Option<bool> {
    let value = output.trim().trim_matches('\'');
    match value {
        "prefer-dark" => Some(true),
        "default" | "prefer-light" => Some(false),
        _ => None,
    }
}

/// `gsettings get org.gnome.desktop.interface gtk-theme` → `'Yaru-dark'`: a theme name that
/// says dark (older GNOME, Cinnamon, MATE, XFCE with the GNOME keys).
pub fn gtk_theme_dark(output: &str) -> bool {
    let name = output.trim().trim_matches('\'').to_ascii_lowercase();
    name.ends_with("-dark") || name.ends_with(":dark") || name.contains("-dark-")
}

/// `gsettings get … high-contrast` or a theme called "HighContrast".
pub fn gsettings_high_contrast(a11y_output: &str, gtk_theme: &str) -> bool {
    a11y_output.trim() == "true"
        || gtk_theme
            .trim()
            .trim_matches('\'')
            .to_ascii_lowercase()
            .starts_with("highcontrast")
}

/// macOS `defaults read -g AppleInterfaceStyle` prints `Dark` in dark mode (and fails in
/// light mode, i.e. empty output here).
pub fn macos_interface_dark(output: &str) -> bool {
    output.trim().eq_ignore_ascii_case("dark")
}

/// macOS `defaults read com.apple.universalaccess increaseContrast` → `1`.
pub fn macos_increase_contrast(output: &str) -> bool {
    output.trim() == "1"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_in_menu_order() {
        let ids: Vec<&str> = Theme::ALL.iter().map(|t| t.id()).collect();
        assert_eq!(ids, ["purikura", "showa", "manga", "yume", "night"]);
        for t in Theme::ALL {
            assert_eq!(Theme::from_id(t.id()), Some(t));
            assert!(!t.label().is_empty());
        }
        assert_eq!(Theme::from_id("Purikura"), None);
        assert_eq!(Theme::from_id(""), None);
        assert!(Theme::Night.is_dark() && !Theme::Manga.is_dark());
    }

    #[test]
    fn suggestion_follows_android_rules() {
        let look = |dark, high_contrast| SystemLook {
            dark,
            high_contrast,
        };
        assert_eq!(suggest(look(false, false)), Theme::Purikura);
        assert_eq!(suggest(look(true, false)), Theme::Night);
        // High contrast wins over dark mode (Manga is the high-contrast theme).
        assert_eq!(suggest(look(true, true)), Theme::Manga);
        assert_eq!(suggest(look(false, true)), Theme::Manga);
    }

    #[test]
    fn a_saved_choice_always_wins() {
        let dark = SystemLook {
            dark: true,
            high_contrast: false,
        };
        assert_eq!(effective("yume", dark), Theme::Yume);
        assert_eq!(effective(" showa ", SystemLook::default()), Theme::Showa);
        assert_eq!(effective("", dark), Theme::Night);
        assert_eq!(effective("kawaii", SystemLook::default()), Theme::Purikura);
    }

    #[test]
    fn system_signals_parse() {
        assert_eq!(gsettings_color_scheme_dark("'prefer-dark'\n"), Some(true));
        assert_eq!(gsettings_color_scheme_dark("'default'\n"), Some(false));
        assert_eq!(gsettings_color_scheme_dark(""), None);
        assert!(gtk_theme_dark("'Yaru-dark'\n"));
        assert!(gtk_theme_dark("'Adwaita:dark'"));
        assert!(gtk_theme_dark("'Mint-Y-Dark-Aqua'"));
        assert!(!gtk_theme_dark("'Yaru'"));
        assert!(!gtk_theme_dark("'Darkroom'"));
        assert!(gsettings_high_contrast("true\n", "'Yaru'"));
        assert!(gsettings_high_contrast("false", "'HighContrast'"));
        assert!(!gsettings_high_contrast("false", "'Yaru'"));
        assert!(macos_interface_dark("Dark\n"));
        assert!(!macos_interface_dark(""));
        assert!(macos_increase_contrast("1\n"));
        assert!(!macos_increase_contrast("0"));
    }
}
