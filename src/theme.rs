//! Theme: UI colors plus the ANSI palette fed to the renderer.
//!
//! Themes are selected by preset name from `[theme]`/`theme = "..."` in the
//! config (SPEC §14, THEME-001/013/014). An unknown preset name falls back
//! to `predawn` and is reported by the caller; themes are validated on load
//! because every preset is a static, compile-checked table.

use crate::color::{Color, ansi_palette};

/// A terminal theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Theme name (preset name).
    pub name: String,
    /// Default background.
    pub background: Color,
    /// Default foreground.
    pub foreground: Color,
    /// Cursor color.
    pub cursor: Color,
    /// Selection highlight background (WIN-004 mouse selection).
    pub selection: Color,
    /// Tab bar strip background (tab bar P0).
    pub tab_bar: Color,
    /// Active tab background.
    pub tab_active: Color,
    /// Pane border between split panes (SPLIT-014).
    pub split_border: Color,
    /// Border of the focused split pane (SPLIT-015).
    pub split_focus: Color,
    /// ANSI-16 palette (indices 0..=15).
    pub palette: [Color; 16],
}

/// Every built-in preset, in the order [`preset_names`] reports them.
pub const PRESETS: [&str; 5] = [
    "predawn",
    "catppuccin-mocha",
    "gruvbox-dark",
    "one-dark",
    "solarized-dark",
];

impl Theme {
    /// The built-in `predawn` preset (SPEC §14 default).
    #[must_use]
    pub fn predawn() -> Self {
        Self {
            name: String::from("predawn"),
            background: Color::from_hex(0x23212E),
            foreground: Color::from_hex(0xEAE6E3),
            cursor: Color::from_hex(0x61C0FF),
            selection: Color::from_hex(0x4C465C),
            tab_bar: Color::from_hex(0x1A1822),
            tab_active: Color::from_hex(0x3A3646),
            split_border: Color::from_hex(0x4C465C),
            split_focus: Color::from_hex(0x61C0FF),
            palette: ansi_palette(),
        }
    }

    /// Catppuccin Mocha: soft dark blue-grey, rosewater cursor.
    #[must_use]
    pub fn catppuccin_mocha() -> Self {
        Self {
            name: String::from("catppuccin-mocha"),
            background: Color::from_hex(0x1E1E2E),
            foreground: Color::from_hex(0xCDD6F4),
            cursor: Color::from_hex(0xF5E0DC),
            selection: Color::from_hex(0x313244),
            tab_bar: Color::from_hex(0x181825),
            tab_active: Color::from_hex(0x313244),
            split_border: Color::from_hex(0x45475A),
            split_focus: Color::from_hex(0xF5E0DC),
            palette: [
                Color::from_hex(0x45475A),
                Color::from_hex(0xF38BA8),
                Color::from_hex(0xA6E3A1),
                Color::from_hex(0xF9E2AF),
                Color::from_hex(0x89B4FA),
                Color::from_hex(0xF5C2E7),
                Color::from_hex(0x94E2D5),
                Color::from_hex(0xBAC2DE),
                Color::from_hex(0x585B70),
                Color::from_hex(0xF38BA8),
                Color::from_hex(0xA6E3A1),
                Color::from_hex(0xF9E2AF),
                Color::from_hex(0x89B4FA),
                Color::from_hex(0xF5C2E7),
                Color::from_hex(0x94E2D5),
                Color::from_hex(0xA6ADC8),
            ],
        }
    }

    /// Gruvbox dark: warm brown, retro-gelish tones.
    #[must_use]
    pub fn gruvbox_dark() -> Self {
        Self {
            name: String::from("gruvbox-dark"),
            background: Color::from_hex(0x282828),
            foreground: Color::from_hex(0xEBDBB2),
            cursor: Color::from_hex(0xEBDBB2),
            selection: Color::from_hex(0x504945),
            tab_bar: Color::from_hex(0x1D2021),
            tab_active: Color::from_hex(0x3C3836),
            split_border: Color::from_hex(0x504945),
            split_focus: Color::from_hex(0xEBDBB2),
            palette: [
                Color::from_hex(0x282828),
                Color::from_hex(0xCC241D),
                Color::from_hex(0x98971A),
                Color::from_hex(0xD79921),
                Color::from_hex(0x458588),
                Color::from_hex(0xB16286),
                Color::from_hex(0x689D6A),
                Color::from_hex(0xA89984),
                Color::from_hex(0x928374),
                Color::from_hex(0xFB4934),
                Color::from_hex(0xB8BB26),
                Color::from_hex(0xFABD2F),
                Color::from_hex(0x83A598),
                Color::from_hex(0xD3869B),
                Color::from_hex(0x8EC07C),
                Color::from_hex(0xEBDBB2),
            ],
        }
    }

    /// One Dark: the Atom editor's dark scheme.
    #[must_use]
    pub fn one_dark() -> Self {
        Self {
            name: String::from("one-dark"),
            background: Color::from_hex(0x282C34),
            foreground: Color::from_hex(0xABB2BF),
            cursor: Color::from_hex(0x528BFF),
            selection: Color::from_hex(0x3E4451),
            tab_bar: Color::from_hex(0x21252B),
            tab_active: Color::from_hex(0x3E4451),
            split_border: Color::from_hex(0x3E4451),
            split_focus: Color::from_hex(0x528BFF),
            palette: [
                Color::from_hex(0x282C34),
                Color::from_hex(0xE06C75),
                Color::from_hex(0x98C379),
                Color::from_hex(0xD19A66),
                Color::from_hex(0x61AFEF),
                Color::from_hex(0xC678DD),
                Color::from_hex(0x56B6C2),
                Color::from_hex(0xABB2BF),
                Color::from_hex(0x5C6370),
                Color::from_hex(0xE06C75),
                Color::from_hex(0x98C379),
                Color::from_hex(0xD19A66),
                Color::from_hex(0x61AFEF),
                Color::from_hex(0xC678DD),
                Color::from_hex(0x56B6C2),
                Color::from_hex(0xFFFFFF),
            ],
        }
    }

    /// Solarized dark: Ethan Schoonover's low-contrast scheme.
    #[must_use]
    pub fn solarized_dark() -> Self {
        Self {
            name: String::from("solarized-dark"),
            background: Color::from_hex(0x002B36),
            foreground: Color::from_hex(0x839496),
            cursor: Color::from_hex(0x93A1A1),
            selection: Color::from_hex(0x073642),
            tab_bar: Color::from_hex(0x00212B),
            tab_active: Color::from_hex(0x073642),
            split_border: Color::from_hex(0x073642),
            split_focus: Color::from_hex(0x93A1A1),
            palette: [
                Color::from_hex(0x002B36),
                Color::from_hex(0xDC322F),
                Color::from_hex(0x859900),
                Color::from_hex(0xB58900),
                Color::from_hex(0x268BD2),
                Color::from_hex(0xD33682),
                Color::from_hex(0x2AA198),
                Color::from_hex(0xEEE8D5),
                Color::from_hex(0x586E75),
                Color::from_hex(0xCB4B16),
                Color::from_hex(0x859900),
                Color::from_hex(0xB58900),
                Color::from_hex(0x268BD2),
                Color::from_hex(0xD33682),
                Color::from_hex(0x2AA198),
                Color::from_hex(0xFDF6E3),
            ],
        }
    }

    /// Every built-in preset name.
    #[must_use]
    pub const fn preset_names() -> &'static [&'static str] {
        &PRESETS
    }

    /// Look up a preset by exact name; `None` when it does not exist.
    #[must_use]
    pub fn preset(name: &str) -> Option<Self> {
        match name {
            "predawn" => Some(Self::predawn()),
            "catppuccin-mocha" => Some(Self::catppuccin_mocha()),
            "gruvbox-dark" => Some(Self::gruvbox_dark()),
            "one-dark" => Some(Self::one_dark()),
            "solarized-dark" => Some(Self::solarized_dark()),
            _ => None,
        }
    }

    /// Resolve a configured theme name, falling back to `predawn` when the
    /// name is unknown (THEME-014: invalid themes fall back safely).
    #[must_use]
    pub fn resolve(name: &str) -> Self {
        Self::preset(name).unwrap_or_else(Self::predawn)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::predawn()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predawn_is_the_default() {
        let theme = Theme::default();
        assert_eq!(theme.name, "predawn");
        assert_eq!(theme.background, Color::from_hex(0x23212E));
        assert_eq!(theme.palette.len(), 16);
    }

    #[test]
    fn every_preset_resolves_with_matching_name() {
        for &name in Theme::preset_names() {
            let theme = Theme::preset(name).unwrap_or_preset(name);
            assert_eq!(theme.name, name, "preset name field must match its key");
            assert_eq!(Theme::resolve(name), theme);
            assert_eq!(theme.palette.len(), 16);
        }
    }

    #[test]
    fn presets_are_distinct() {
        let names = Theme::preset_names();
        for (i, &a) in names.iter().enumerate() {
            for &b in &names[i + 1..] {
                let ta = Theme::preset(a).unwrap_or_preset(a);
                let tb = Theme::preset(b).unwrap_or_preset(b);
                assert_ne!(
                    ta.background, tb.background,
                    "{a} and {b} share a background"
                );
            }
        }
    }

    #[test]
    fn unknown_preset_falls_back_to_predawn() {
        assert_eq!(Theme::preset("not-a-theme"), None);
        assert_eq!(Theme::resolve("not-a-theme"), Theme::predawn());
    }

    trait UnwrapPreset {
        fn unwrap_or_preset(self, name: &str) -> Theme;
    }

    impl UnwrapPreset for Option<Theme> {
        fn unwrap_or_preset(self, name: &str) -> Theme {
            self.unwrap_or_else(|| panic!("preset {name} missing"))
        }
    }
}
