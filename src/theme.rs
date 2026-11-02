//! Theme: UI colors plus the ANSI palette fed to the renderer.
//!
//! A theme is a preset name (`theme = "gruvbox-dark"`) and/or a table of
//! colour overrides (`[theme]`, CONFIG-003), optionally read from a named
//! theme file `themes/<name>.toml` next to the config (THEME-009). Unknown
//! preset names fall back to `predawn` and every invalid colour is reported
//! and skipped, keeping the value it already had (THEME-013/014).

use std::path::Path;

use serde::Deserialize;

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
    /// Search match highlight background (SEARCH-004).
    pub search_highlight: Color,
    /// Tab bar strip background (tab bar P0).
    pub tab_bar: Color,
    /// Active tab background.
    pub tab_active: Color,
    /// Pane border between split panes (SPLIT-014).
    pub split_border: Color,
    /// Border of the focused split pane (SPLIT-015).
    pub split_focus: Color,
    /// OSC 8 hyperlink text color (issue #41, SEC-022).
    pub link: Color,
    /// ANSI-16 palette (indices 0..=15).
    pub palette: [Color; 16],
}

/// Every built-in preset, in the order [`preset_names`] reports them.
pub const PRESETS: [&str; 6] = [
    "predawn",
    "catppuccin-mocha",
    "gruvbox-dark",
    "one-dark",
    "solarized-dark",
    "solarized-light",
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
            search_highlight: Color::from_hex(0x4C465C),
            tab_bar: Color::from_hex(0x1A1822),
            tab_active: Color::from_hex(0x3A3646),
            split_border: Color::from_hex(0x4C465C),
            split_focus: Color::from_hex(0x61C0FF),
            link: Color::from_hex(0x61C0FF),
            palette: ansi_palette(),
        }
    }

    /// Solarized light: the light half of the Solarized pair (THEME-010).
    #[must_use]
    pub fn solarized_light() -> Self {
        Self {
            name: String::from("solarized-light"),
            background: Color::from_hex(0xFDF6E3),
            foreground: Color::from_hex(0x586E75),
            cursor: Color::from_hex(0x268BD2),
            selection: Color::from_hex(0xEAE3C9),
            search_highlight: Color::from_hex(0xEAE3C9),
            tab_bar: Color::from_hex(0xEDE8D3),
            tab_active: Color::from_hex(0xEAE3C9),
            split_border: Color::from_hex(0xEAE3C9),
            split_focus: Color::from_hex(0x268BD2),
            link: Color::from_hex(0x268BD2),
            palette: [
                Color::from_hex(0x93A1A1),
                Color::from_hex(0xDC322F),
                Color::from_hex(0x859900),
                Color::from_hex(0xB58900),
                Color::from_hex(0x268BD2),
                Color::from_hex(0xD33682),
                Color::from_hex(0x2AA198),
                Color::from_hex(0xEEE8D5),
                Color::from_hex(0x002B36),
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

    /// The preset that matches the system's dark/light setting (THEME-010).
    /// An unreadable registry gives the built-in default.
    #[must_use]
    pub fn system_preset() -> &'static str {
        #[cfg(windows)]
        {
            use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_DWORD, RegGetValueW};
            use windows::core::w;
            let mut value = 0u32;
            let mut size = 4u32;
            let status =
                // SAFETY: FFI boundary (NFR-401) - a 4-byte out buffer and its size.
                unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
                    w!("AppsUseLightTheme"),
                    RRF_RT_DWORD,
                    None::<*mut windows::Win32::System::Registry::REG_VALUE_TYPE>,
                    Some(std::ptr::addr_of_mut!(value).cast::<core::ffi::c_void>()),
                    Some(std::ptr::addr_of_mut!(size)),
                )
            };
            if status.is_ok() && value != 0 {
                "solarized-light"
            } else {
                crate::config::DEFAULT_THEME
            }
        }
        #[cfg(not(windows))]
        {
            crate::config::DEFAULT_THEME
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
            search_highlight: Color::from_hex(0x313244),
            tab_bar: Color::from_hex(0x181825),
            tab_active: Color::from_hex(0x313244),
            split_border: Color::from_hex(0x45475A),
            split_focus: Color::from_hex(0xF5E0DC),
            link: Color::from_hex(0x89B4FA),
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
            search_highlight: Color::from_hex(0x504945),
            tab_bar: Color::from_hex(0x1D2021),
            tab_active: Color::from_hex(0x3C3836),
            split_border: Color::from_hex(0x504945),
            split_focus: Color::from_hex(0xEBDBB2),
            link: Color::from_hex(0x83A598),
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
            search_highlight: Color::from_hex(0x3E4451),
            tab_bar: Color::from_hex(0x21252B),
            tab_active: Color::from_hex(0x3E4451),
            split_border: Color::from_hex(0x3E4451),
            split_focus: Color::from_hex(0x528BFF),
            link: Color::from_hex(0x61AFEF),
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
            search_highlight: Color::from_hex(0x073642),
            tab_bar: Color::from_hex(0x00212B),
            tab_active: Color::from_hex(0x073642),
            split_border: Color::from_hex(0x073642),
            split_focus: Color::from_hex(0x93A1A1),
            link: Color::from_hex(0x268BD2),
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
            "solarized-light" => Some(Self::solarized_light()),
            _ => None,
        }
    }

    /// Resolve a configured theme name, falling back to `predawn` when the
    /// name is unknown (THEME-014: invalid themes fall back safely).
    #[must_use]
    pub fn resolve(name: &str) -> Self {
        Self::preset(name).unwrap_or_else(Self::predawn)
    }

    /// Apply a table of colour overrides (CONFIG-003). Each key that parses
    /// wins; each key that does not is returned as a problem and leaves the
    /// existing color alone (THEME-014).
    pub fn apply_overrides(&mut self, file: &ThemeFile) -> Vec<String> {
        let mut problems = Vec::new();
        let mut apply = |key: &str, from: &Option<String>, to: &mut Color| {
            let Some(text) = from else { return };
            match Color::parse_hex(text) {
                Some(color) => *to = color,
                None => problems.push(format!("[theme] {key} = {text:?} is not #RRGGBB")),
            }
        };
        apply("background", &file.background, &mut self.background);
        apply("foreground", &file.foreground, &mut self.foreground);
        apply("cursor", &file.cursor, &mut self.cursor);
        apply("selection", &file.selection, &mut self.selection);
        apply(
            "search-highlight",
            &file.search_highlight,
            &mut self.search_highlight,
        );
        apply("tab-bar", &file.tab_bar, &mut self.tab_bar);
        apply("tab-active", &file.tab_active, &mut self.tab_active);
        apply("split-border", &file.split_border, &mut self.split_border);
        apply("split-focus", &file.split_focus, &mut self.split_focus);
        apply("link", &file.link, &mut self.link);
        if let Some(palette) = &file.palette {
            // THEME-013: the palette is all-or-nothing, a short list would
            // silently repaint one ANSI color of the preset.
            if palette.len() != 16 {
                problems.push(format!(
                    "[theme] palette has {} colors, expected 16",
                    palette.len()
                ));
            } else {
                let mut parsed = [ansi_palette()[0]; 16];
                let mut ok = true;
                for (i, text) in palette.iter().enumerate() {
                    match Color::parse_hex(text) {
                        Some(color) => parsed[i] = color,
                        None => {
                            problems
                                .push(format!("[theme] palette[{i}] = {text:?} is not #RRGGBB"));
                            ok = false;
                        }
                    }
                }
                if ok {
                    self.palette = parsed;
                }
            }
        }
        problems
    }

    /// Read `theme_dir/<name>.toml` (THEME-009). The error text says what is
    /// wrong so the caller can report it and carry on with a preset.
    pub fn load_file(theme_dir: &Path, name: &str) -> Result<ThemeFile, String> {
        if name.is_empty() || name.contains(['/', '\\', '.']) {
            return Err(format!("{name:?} is not a theme name"));
        }
        let path = theme_dir.join(format!("{name}.toml"));
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Colour overrides for one theme: the `[theme]` table in the config, or the
/// whole of a `themes/<name>.toml` file. Every key is optional, so a theme
/// file only states what it changes (CONFIG-003, THEME-009).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ThemeFile {
    /// Name this theme answers to (`name` in the table, CONFIG-002).
    pub name: Option<String>,
    /// Default background.
    pub background: Option<String>,
    /// Default foreground.
    pub foreground: Option<String>,
    /// Cursor color.
    pub cursor: Option<String>,
    /// Selection background.
    pub selection: Option<String>,
    /// Search match background.
    pub search_highlight: Option<String>,
    /// Tab bar strip background.
    pub tab_bar: Option<String>,
    /// Active tab background.
    pub tab_active: Option<String>,
    /// Split pane border.
    pub split_border: Option<String>,
    /// Focused split pane border.
    pub split_focus: Option<String>,
    /// OSC 8 hyperlink text color (issue #41).
    pub link: Option<String>,
    /// ANSI-16 palette, 16 `#RRGGBB` strings.
    pub palette: Option<Vec<String>>,
}

/// Resolve the theme a config asks for: preset by name, then the named theme
/// file in `theme_dir`, then the `[theme]` overrides on top. Unknown names
/// fall back to `predawn`; every problem found is returned for the caller to
/// report (THEME-009/013/014, CONFIG-003).
#[must_use]
pub fn resolve_configured(
    name: &str,
    overrides: &ThemeFile,
    theme_dir: Option<&Path>,
) -> (Theme, Vec<String>) {
    let mut problems = Vec::new();
    let mut theme = Theme::preset(name).unwrap_or_else(Theme::predawn);
    let mut from_file = false;
    if let Some(dir) = theme_dir {
        match Theme::load_file(dir, name) {
            Ok(file) => {
                from_file = true;
                problems.extend(theme.apply_overrides(&file));
            }
            Err(e) => {
                // Only a file that exists and fails to parse is a problem; a
                // preset simply has no file.
                if dir.join(format!("{name}.toml")).exists() {
                    problems.push(format!("theme file {e}"));
                }
            }
        }
    }
    if from_file {
        // A theme file is a theme of its own, not a patch on predawn.
        theme.name = name.to_string();
    }
    if theme.name != name && !name.is_empty() && !from_file {
        problems.push(format!(
            "unknown theme {name:?} (known: {}), using predawn",
            PRESETS.join(", ")
        ));
    }
    problems.extend(theme.apply_overrides(overrides));
    if let Some(name) = &overrides.name {
        theme.name = name.clone();
    }
    (theme, problems)
}

/// Theme file names (the stems of `*.toml`) in a theme directory, sorted.
/// The View→Themes menu lists these after the presets (roadmap v0.2
/// "preset browser"). A missing directory simply contributes nothing.
#[must_use]
pub fn list_files(theme_dir: Option<&Path>) -> Vec<String> {
    let Some(dir) = theme_dir else {
        return Vec::new();
    };
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            (path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("toml")))
            .then(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .flatten()
        })
        .collect();
    names.sort();
    names
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

    fn table(text: &str) -> ThemeFile {
        toml::from_str(text).unwrap_or_else(|e| panic!("bad theme table: {e}"))
    }

    #[test]
    fn the_light_preset_is_light_and_the_system_preset_is_known() {
        let light = Theme::preset("solarized-light").expect("preset exists");
        assert!(
            light.background.r > light.foreground.r,
            "the light preset has a light background"
        );
        assert!(Theme::preset_names().contains(&"solarized-light"));
        assert!(
            Theme::preset_names().contains(&Theme::system_preset()),
            "the system preset is a real preset"
        );
    }

    #[test]
    fn a_colour_override_wins_and_a_bad_one_is_reported() {
        let base = Theme::predawn();
        let mut theme = base.clone();
        let problems = theme.apply_overrides(&table(
            "background = \"1A1B1C\"\ncursor = \"not-a-color\"\ntab-bar = \"#FF0000\"\n",
        ));
        assert_eq!(theme.background, Color::rgb(0x1A, 0x1B, 0x1C));
        assert_eq!(theme.tab_bar, Color::rgb(0xFF, 0x00, 0x00));
        assert_eq!(theme.cursor, base.cursor, "invalid color keeps preset");
        assert_eq!(problems.len(), 1, "one problem: {problems:?}");
    }

    #[test]
    fn the_palette_is_all_or_nothing() {
        let base = Theme::predawn();
        let mut theme = base.clone();
        let short = theme.apply_overrides(&table("palette = [\"#000000\"]"));
        assert_eq!(theme.palette, base.palette);
        assert_eq!(short.len(), 1, "a 1-color palette is a problem: {short:?}");

        let full = (0..16)
            .map(|i| format!("\"#{i:02X}0000\""))
            .collect::<Vec<_>>()
            .join(", ");
        let problems = theme.apply_overrides(&table(&format!("palette = [{full}]")));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(theme.palette[3], Color::rgb(0x03, 0x00, 0x00));
    }

    #[test]
    fn an_unknown_theme_key_is_rejected() {
        // THEME-013: a typo in a theme file is a problem, not silence.
        assert!(toml::from_str::<ThemeFile>("backround = \"#000000\"").is_err());
    }

    #[test]
    fn a_named_theme_file_overrides_the_preset() {
        let dir = std::env::temp_dir().join(format!("rustty-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        let path = dir.join("dracula-ish.toml");
        std::fs::write(
            &path,
            "background = \"#282A36\"\ntab-active = \"#44475A\"\n",
        )
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));

        let (theme, problems) =
            resolve_configured("dracula-ish", &ThemeFile::default(), Some(&dir));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(theme.name, "dracula-ish");
        assert_eq!(theme.background, Color::rgb(0x28, 0x2A, 0x36));
        assert_eq!(theme.tab_active, Color::rgb(0x44, 0x47, 0x5A));
    }

    #[test]
    fn the_config_table_wins_over_the_theme_file() {
        let dir = std::env::temp_dir().join(format!("rustty-theme2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("grid.toml"), "foreground = \"#111111\"\n").unwrap();
        let table = table("name = \"grid\"\nforeground = \"#222222\"\n");

        let (theme, problems) = resolve_configured("gruvbox-dark", &table, Some(&dir));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(theme.name, "grid");
        assert_eq!(theme.background, Theme::gruvbox_dark().background);
        assert_eq!(theme.foreground, Color::rgb(0x22, 0x22, 0x22));
    }

    #[test]
    fn a_missing_theme_name_falls_back_and_says_so() {
        let (theme, problems) = resolve_configured("nope", &ThemeFile::default(), None);
        assert_eq!(theme, Theme::predawn());
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn theme_files_are_listed_sorted_by_stem() {
        let dir = std::env::temp_dir().join(format!("rustty-theme3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        std::fs::write(dir.join("zeta.toml"), "").unwrap();
        std::fs::write(dir.join("alpha.toml"), "").unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();

        let mut names = list_files(Some(&dir));
        names.retain(|n| n == "alpha" || n == "zeta");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(names, vec!["alpha".to_string(), "zeta".to_string()]);
        assert_eq!(list_files(None), Vec::<String>::new());
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
