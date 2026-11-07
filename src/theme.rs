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
    /// Optional UI-only colors, independent of terminal text and ANSI output.
    pub ui: UiOverrides,
    /// ANSI-16 palette (indices 0..=15).
    pub palette: [Color; 16],
}

/// UI-only color overrides. Missing colors follow the terminal theme's chrome.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiOverrides {
    /// Primary chrome text.
    pub foreground: Option<Color>,
    /// Secondary labels and disabled-state explanations.
    pub muted: Option<Color>,
    /// Positional focus indicators.
    pub accent: Option<Color>,
    /// Chrome separators.
    pub border: Option<Color>,
    /// Tab strip fill, set only by an explicit `tab-bar` key. Without one
    /// the strip matches the terminal background, so the window reads as
    /// one surface.
    pub strip: Option<Color>,
}

/// Resolved semantic chrome colors; cheap to compute and allocation-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiColors {
    /// Primary chrome text.
    pub foreground: Color,
    /// Secondary labels and disabled-state explanations.
    pub muted: Color,
    /// Positional focus indicators.
    pub accent: Color,
    /// Chrome separators.
    pub border: Color,
}

/// Every built-in preset, in the order [`preset_names`] reports them.
pub const PRESETS: [&str; 8] = [
    "predawn",
    "catppuccin-mocha",
    "gruvbox-dark",
    "one-dark",
    "solarized-dark",
    "solarized-light",
    "copper-dark",
    "copper-light",
];

impl Theme {
    /// The built-in `predawn` preset (SPEC §14 default).
    #[must_use]
    pub fn predawn() -> Self {
        Self {
            name: String::from("predawn"),
            ui: UiOverrides::default(),
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
            ui: UiOverrides::default(),
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
                // Solarized's own white and bright white are this theme's background
                // tones, and shells type in them. These are its readable grays.
                Color::from_hex(0x657B83),
                Color::from_hex(0x002B36),
                Color::from_hex(0xCB4B16),
                Color::from_hex(0x859900),
                Color::from_hex(0xB58900),
                Color::from_hex(0x268BD2),
                Color::from_hex(0xD33682),
                Color::from_hex(0x2AA198),
                Color::from_hex(0x586E75),
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
            system_preset_for(status.is_ok() && value != 0)
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
            ui: UiOverrides::default(),
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
            ui: UiOverrides::default(),
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
            ui: UiOverrides::default(),
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
            ui: UiOverrides::default(),
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

    /// Copper native: graphite surfaces, warm text and restrained copper accents.
    #[must_use]
    pub fn copper_dark() -> Self {
        Self {
            name: "copper-dark".into(),
            background: Color::from_hex(0x17191C),
            foreground: Color::from_hex(0xE7E2DB),
            cursor: Color::from_hex(0xE0A477),
            selection: Color::from_hex(0x41464F),
            search_highlight: Color::from_hex(0x55402F),
            tab_bar: Color::from_hex(0x202328),
            tab_active: Color::from_hex(0x202328),
            split_border: Color::from_hex(0x3D424A),
            split_focus: Color::from_hex(0xE0A477),
            link: Color::from_hex(0x8CBCEB),
            ui: UiOverrides {
                strip: None,
                foreground: Some(Color::from_hex(0xF0EAE2)),
                muted: Some(Color::from_hex(0xB6B0A8)),
                accent: Some(Color::from_hex(0xE0A477)),
                border: Some(Color::from_hex(0x454B54)),
            },
            palette: [
                0x25282D, 0xE58F88, 0xA3C58C, 0xDDBA79, 0x8CBCEB, 0xC5A2D6, 0x88C6C2, 0xD5D0C8,
                0x92979F, 0xF3ABA3, 0xBCDBA5, 0xEED29B, 0xADD2F5, 0xDCC1E9, 0xA5DEDA, 0xF5F0E9,
            ]
            .map(Color::from_hex),
        }
    }

    /// Copper native light: warm paper and ink with a deep copper focus accent.
    #[must_use]
    pub fn copper_light() -> Self {
        Self {
            name: "copper-light".into(),
            background: Color::from_hex(0xFAF7F2),
            foreground: Color::from_hex(0x292D32),
            cursor: Color::from_hex(0x924923),
            selection: Color::from_hex(0xD6DDE5),
            search_highlight: Color::from_hex(0xE9CDAF),
            tab_bar: Color::from_hex(0xF0EBE4),
            tab_active: Color::from_hex(0xFFFCF8),
            split_border: Color::from_hex(0xB9B1A6),
            split_focus: Color::from_hex(0x924923),
            link: Color::from_hex(0x245E91),
            ui: UiOverrides {
                strip: None,
                foreground: Some(Color::from_hex(0x292D32)),
                muted: Some(Color::from_hex(0x655D54)),
                accent: Some(Color::from_hex(0x924923)),
                border: Some(Color::from_hex(0xB9B1A6)),
            },
            palette: [
                // White and bright white are dark here: shells type in them, and on a
                // light background the usual pale values cannot be read.
                0x292D32, 0xA53236, 0x3E652C, 0x785817, 0x245E91, 0x754589, 0x1D6867, 0x5F5A53,
                0x69635C, 0xAF3539, 0x426D30, 0x805C14, 0x28679E, 0x814A94, 0x207171, 0x292D32,
            ]
            .map(Color::from_hex),
        }
    }

    /// What each color written under this theme becomes under `new` (THEME-016).
    /// The theme's own text and background come first, so they win when a palette
    /// entry happens to share their color.
    #[must_use]
    pub fn recolor_map(&self, new: &Self) -> Vec<(Color, Color)> {
        let mut map = vec![
            (self.foreground, new.foreground),
            (self.background, new.background),
        ];
        map.extend(self.palette.iter().copied().zip(new.palette));
        map
    }

    /// Resolve UI colors without changing the terminal's ANSI palette or cursor.
    #[must_use]
    pub fn ui_colors(&self) -> UiColors {
        let foreground = self.ui.foreground.unwrap_or(self.foreground);
        let blend = |fg: u8, bg: u8| ((u16::from(fg) * 3 + u16::from(bg)) / 4) as u8;
        UiColors {
            foreground,
            muted: self.ui.muted.unwrap_or(Color::rgb(
                blend(foreground.r, self.tab_bar.r),
                blend(foreground.g, self.tab_bar.g),
                blend(foreground.b, self.tab_bar.b),
            )),
            accent: self.ui.accent.unwrap_or(self.split_focus),
            border: self.ui.border.unwrap_or(self.split_border),
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
            "copper-dark" => Some(Self::copper_dark()),
            "copper-light" => Some(Self::copper_light()),
            _ => None,
        }
    }

    /// Fill for the tab strip: the terminal background unless the user set
    /// the `tab-bar` key.
    #[must_use]
    pub fn strip_color(&self) -> Color {
        self.ui.strip.unwrap_or(self.background)
    }

    /// Resolve a configured theme name, falling back to the default when the
    /// name is unknown (THEME-014: invalid themes fall back safely).
    #[must_use]
    pub fn resolve(name: &str) -> Self {
        Self::preset(name).unwrap_or_default()
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
        if let Some(color) = file.tab_bar.as_deref().and_then(Color::parse_hex) {
            self.ui.strip = Some(color);
        }
        apply("tab-active", &file.tab_active, &mut self.tab_active);
        apply("split-border", &file.split_border, &mut self.split_border);
        apply("split-focus", &file.split_focus, &mut self.split_focus);
        apply("link", &file.link, &mut self.link);
        // Preserve absent overrides so legacy themes continue to follow their
        // terminal foreground. Resolve fallback colors only when drawing.
        for (key, from, to) in [
            (
                "ui-foreground",
                &file.ui_foreground,
                &mut self.ui.foreground,
            ),
            ("ui-muted", &file.ui_muted, &mut self.ui.muted),
            ("ui-accent", &file.ui_accent, &mut self.ui.accent),
            ("ui-border", &file.ui_border, &mut self.ui.border),
        ] {
            if let Some(text) = from {
                if let Some(color) = Color::parse_hex(text) {
                    *to = Some(color);
                } else {
                    problems.push(format!("[theme] {key} = {text:?} is not #RRGGBB"));
                }
            }
        }
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
    /// Primary chrome text, separate from terminal foreground (SKIN-002).
    pub ui_foreground: Option<String>,
    /// Secondary labels and disabled-state explanations.
    pub ui_muted: Option<String>,
    /// Positional focus indicators, separate from the terminal cursor.
    pub ui_accent: Option<String>,
    /// Chrome separators.
    pub ui_border: Option<String>,
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
    let mut theme = Theme::preset(name).unwrap_or_default();
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

/// The preset for a light or dark system (THEME-010).
#[must_use]
pub const fn system_preset_for(light: bool) -> &'static str {
    if light {
        "copper-light"
    } else {
        crate::config::DEFAULT_THEME
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::copper_dark()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copper_dark_is_the_default() {
        let theme = Theme::default();
        assert_eq!(theme.name, "copper-dark");
        assert_eq!(theme.background, Color::from_hex(0x17191C));
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
    fn default_and_unknown_themes_are_copper_dark() {
        assert_eq!(Theme::preset("not-a-theme"), None);
        assert_eq!(Theme::resolve("not-a-theme"), Theme::copper_dark());
        assert_eq!(Theme::default(), Theme::copper_dark());
        assert_eq!(crate::config::DEFAULT_THEME, "copper-dark");
    }

    #[test]
    fn light_system_maps_to_copper_light() {
        assert_eq!(system_preset_for(true), "copper-light");
        assert_eq!(system_preset_for(false), "copper-dark");
    }

    #[test]
    fn strip_is_the_background_unless_tab_bar_is_set_explicitly() {
        // Break caught: a band above the text on presets whose tab_bar
        // differs from the background, or an ignored user tab-bar key.
        for name in PRESETS {
            let theme = Theme::preset(name).expect("preset");
            assert_eq!(theme.strip_color(), theme.background, "{name}");
        }
        let (theme, problems) =
            resolve_configured("copper-dark", &table("tab-bar = \"#010203\"\n"), None);
        assert!(problems.is_empty());
        assert_eq!(theme.strip_color(), Color::rgb(1, 2, 3));
    }

    fn table(text: &str) -> ThemeFile {
        toml::from_str(text).unwrap_or_else(|e| panic!("bad theme table: {e}"))
    }

    #[test]
    fn white_text_stays_readable_on_light_themes() {
        // Break caught: PowerShell types in bright white, which Solarized Light drew
        // in its own background color, so what you typed disappeared.
        for theme in [Theme::solarized_light(), Theme::copper_light()] {
            for ansi in [7, 15] {
                let ratio = contrast(theme.palette[ansi], theme.background);
                assert!(ratio >= 3.0, "{} {ansi}: {ratio:.2}", theme.name);
            }
        }
    }

    #[test]
    fn a_recolor_map_covers_the_palette_and_the_default_colors() {
        let (old, new) = (Theme::copper_dark(), Theme::solarized_light());
        let map = old.recolor_map(&new);
        for ansi in 0..16 {
            assert!(
                map.contains(&(old.palette[ansi], new.palette[ansi])),
                "{ansi}"
            );
        }
        assert_eq!(map[0], (old.foreground, new.foreground));
        assert_eq!(map[1], (old.background, new.background));
    }

    fn contrast(a: Color, b: Color) -> f64 {
        let luminance = |c: Color| {
            let linear = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn copper_text_and_focus_meet_contrast_contract() {
        for theme in [Theme::copper_dark(), Theme::copper_light()] {
            let ui = theme.ui_colors();
            for bg in [theme.tab_bar, theme.tab_active, theme.background] {
                for fg in [ui.foreground, ui.muted] {
                    assert!(contrast(fg, bg) >= 4.5, "{}: {fg} on {bg}", theme.name);
                }
                assert!(contrast(ui.accent, bg) >= 3.0, "{} focus", theme.name);
            }
            for bg in [theme.background, theme.selection, theme.search_highlight] {
                assert!(
                    contrast(theme.foreground, bg) >= 4.5,
                    "{} terminal",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn ui_overrides_are_isolated_and_invalid_values_keep_the_preset() {
        let mut theme = Theme::copper_dark();
        let base = theme.clone();
        let problems = theme.apply_overrides(&table(
            "ui-foreground = '#112233'\nui-muted = '#445566'\nui-accent = '#778899'\nui-border = 'bad'",
        ));
        assert_eq!(problems.len(), 1);
        assert_eq!(theme.ui_colors().foreground, Color::from_hex(0x112233));
        assert_eq!(theme.ui_colors().muted, Color::from_hex(0x445566));
        assert_eq!(theme.ui_colors().accent, Color::from_hex(0x778899));
        assert_eq!(theme.ui_colors().border, base.ui_colors().border);
        assert_eq!(theme.foreground, base.foreground);
        assert_eq!(theme.cursor, base.cursor);
        assert_eq!(theme.palette, base.palette);
        let ui = theme.ui_colors();
        theme.apply_overrides(&table("foreground = '#FFFFFF'\ncursor = '#000000'"));
        assert_eq!(theme.ui_colors(), ui);
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
        assert_eq!(theme, Theme::copper_dark());
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
