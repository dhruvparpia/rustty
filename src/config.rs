//! TOML configuration (SPEC §13 schema, §14 defaults).
//!
//! User file: `%APPDATA%\rusttyterm\config.toml` on Windows; in portable
//! mode (see [`crate::portable`]) the `config.toml` beside the executable
//! wins (CONFIG-020). Missing or invalid files fall back to defaults;
//! invalid values are reported by [`Config::load`]. Value ranges are
//! validated on parse (CONFIG-001..006): out-of-range values are errors,
//! not silent clamps.

use std::cmp::Ordering;
use std::io;
use std::path::PathBuf;

use serde::Deserialize;

/// Default font family (SPEC §13 `font`).
pub const DEFAULT_FONT: &str = "Cascadia Mono";

/// Default font size in points (SPEC §13 `font-size`).
pub const DEFAULT_FONT_SIZE: f32 = 11.0;

/// Default line height as a fraction of the glyph box (SPEC §13
/// `line-height`).
pub const DEFAULT_LINE_HEIGHT: f32 = 1.2;

/// Default theme preset name (SPEC §14).
pub const DEFAULT_THEME: &str = "predawn";

/// Default scrollback depth (matches [`Grid::new`](crate::grid::Grid::new)).
pub const DEFAULT_SCROLLBACK: usize = 1000;

/// Smallest accepted `font-size` in points.
pub const MIN_FONT_SIZE: f32 = 5.0;

/// Largest accepted `font-size` in points.
pub const MAX_FONT_SIZE: f32 = 72.0;

/// Smallest accepted `line-height`.
pub const MIN_LINE_HEIGHT: f32 = 0.5;

/// Largest accepted `line-height`.
pub const MAX_LINE_HEIGHT: f32 = 4.0;

/// Largest accepted `scrollback` depth in lines.
pub const MAX_SCROLLBACK: usize = 1_000_000;

/// Largest accepted `[window] cols`.
pub const MAX_COLS: usize = 4000;

/// Largest accepted `[window] rows`.
pub const MAX_ROWS: usize = 1000;

/// Tab-bar visibility (SPEC §13 `tab-bar`, TAB-010 "hide-tab-bar setting").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TabBar {
    /// Always show the tab bar (default).
    #[default]
    Always,
    /// Show the tab bar only when more than one tab is open.
    Auto,
    /// Never show the tab bar (tabs switch with keybindings).
    Hidden,
}

/// Font settings as the renderer consumes them (SPEC FONT-002/008).
///
/// Built from the top-level `font`/`font-size`/`line-height` keys of
/// [`Config`] by [`Config::font`].
#[derive(Debug, Clone)]
pub struct FontConfig {
    /// Preferred family; missing families fall back through the
    /// Cascadia/Consolas chain (FONT-018).
    pub family: String,
    /// Face size in points.
    pub size: f32,
    /// Cell height as a fraction of the measured glyph box.
    pub line_height: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: String::from(DEFAULT_FONT),
            size: DEFAULT_FONT_SIZE,
            line_height: DEFAULT_LINE_HEIGHT,
        }
    }
}

// f32 fields cannot derive Eq; compare floats with `total_cmp`, which is a
// total order (so the resulting relation is reflexive, symmetric, transitive).
impl PartialEq for FontConfig {
    fn eq(&self, other: &Self) -> bool {
        self.family == other.family
            && self.size.total_cmp(&other.size) == Ordering::Equal
            && self.line_height.total_cmp(&other.line_height) == Ordering::Equal
    }
}

impl Eq for FontConfig {}

/// Window/geometry settings (`[window]` section).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowConfig {
    /// Initial width in columns (default 80).
    #[serde(default = "default_cols")]
    pub cols: usize,
    /// Initial height in rows (default 24).
    #[serde(default = "default_rows")]
    pub rows: usize,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            cols: default_cols(),
            rows: default_rows(),
        }
    }
}

fn default_cols() -> usize {
    80
}

/// Default rows (matches [`Grid::new`](crate::grid::Grid::new)).
fn default_rows() -> usize {
    24
}

/// The full configuration tree (SPEC §13 top-level fields).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Theme preset name (SPEC `theme`).
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Font family (SPEC `font`).
    #[serde(default = "default_font")]
    pub font: String,
    /// Font size in points (SPEC `font-size`).
    #[serde(rename = "font-size", default = "default_font_size")]
    pub font_size: f32,
    /// Cell line height (SPEC `line-height`).
    #[serde(rename = "line-height", default = "default_line_height")]
    pub line_height: f32,
    /// Scrollback depth in lines (SPEC `scrollback`).
    #[serde(default = "default_scrollback")]
    pub scrollback: usize,
    /// Tab-bar visibility (SPEC `tab-bar`).
    #[serde(rename = "tab-bar", default)]
    pub tab_bar: TabBar,
    /// `[window]` section.
    #[serde(default)]
    pub window: WindowConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            font: default_font(),
            font_size: default_font_size(),
            line_height: default_line_height(),
            scrollback: default_scrollback(),
            tab_bar: TabBar::default(),
            window: WindowConfig {
                cols: default_cols(),
                rows: default_rows(),
            },
        }
    }
}

// `font_size`/`line_height` are f32: derive(Eq) is impossible, so compare
// floats with `total_cmp` (see the FontConfig note above).
impl PartialEq for Config {
    fn eq(&self, other: &Self) -> bool {
        self.theme == other.theme
            && self.font == other.font
            && self.font_size.total_cmp(&other.font_size) == Ordering::Equal
            && self.line_height.total_cmp(&other.line_height) == Ordering::Equal
            && self.scrollback == other.scrollback
            && self.tab_bar == other.tab_bar
            && self.window == other.window
    }
}

impl Eq for Config {}

fn default_theme() -> String {
    String::from(DEFAULT_THEME)
}

fn default_font() -> String {
    String::from(DEFAULT_FONT)
}

fn default_font_size() -> f32 {
    DEFAULT_FONT_SIZE
}

fn default_line_height() -> f32 {
    DEFAULT_LINE_HEIGHT
}

fn default_scrollback() -> usize {
    DEFAULT_SCROLLBACK
}

impl Config {
    /// The font settings for the renderer, as one bundle.
    #[must_use]
    pub fn font(&self) -> FontConfig {
        FontConfig {
            family: self.font.clone(),
            size: self.font_size,
            line_height: self.line_height,
        }
    }

    /// Parse a configuration string and validate every value (THEME-013
    /// validates on load; out-of-range values are rejected, not clamped).
    ///
    /// # Errors
    ///
    /// Returns an error when the TOML is malformed, a value has the wrong
    /// type, an unknown key appears, or a value is out of its accepted range.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let cfg: Self = toml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Check every value against its accepted range.
    ///
    /// # Errors
    ///
    /// Names the offending key and its accepted range.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.font.trim().is_empty() {
            anyhow::bail!("font: family must not be empty");
        }
        if !(MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&self.font_size) {
            anyhow::bail!("font-size: must be between {MIN_FONT_SIZE} and {MAX_FONT_SIZE} points");
        }
        if !(MIN_LINE_HEIGHT..=MAX_LINE_HEIGHT).contains(&self.line_height) {
            anyhow::bail!("line-height: must be between {MIN_LINE_HEIGHT} and {MAX_LINE_HEIGHT}");
        }
        if self.scrollback > MAX_SCROLLBACK {
            anyhow::bail!("scrollback: must be at most {MAX_SCROLLBACK} lines");
        }
        if self.window.cols == 0 || self.window.cols > MAX_COLS {
            anyhow::bail!("window.cols: must be between 1 and {MAX_COLS}");
        }
        if self.window.rows == 0 || self.window.rows > MAX_ROWS {
            anyhow::bail!("window.rows: must be between 1 and {MAX_ROWS}");
        }
        Ok(())
    }

    /// Load from the user config file, falling back to defaults.
    ///
    /// A missing file is not an error. A present-but-invalid file is.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read, parsed,
    /// or validated.
    pub fn load() -> anyhow::Result<Self> {
        let Some(path) = config_path() else {
            return Ok(Self::default());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(anyhow::Error::new(e).context(format!("reading {}", path.display()))),
        }
    }
}

/// Path of the user config file, or `None` when `%APPDATA%` is unset.
///
/// Portable mode wins over `%APPDATA%` (CONFIG-020, PKG-004): when the
/// executable sits in a portable folder (see [`crate::portable`]), the
/// `config.toml` beside it is used.
#[must_use]
pub fn config_path() -> Option<PathBuf> {
    crate::portable::running_config_path().or_else(|| {
        std::env::var_os("APPDATA")
            .map(|dir| PathBuf::from(dir).join("rusttyterm").join("config.toml"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_uses_defaults() {
        let cfg = Config::parse("").expect_result();
        assert_eq!(cfg, Config::default());
        assert_eq!(cfg.theme, "predawn");
        assert_eq!(cfg.font, DEFAULT_FONT);
        assert_eq!(cfg.font_size, DEFAULT_FONT_SIZE);
        assert_eq!(cfg.scrollback, DEFAULT_SCROLLBACK);
        assert_eq!(cfg.tab_bar, TabBar::Always);
    }

    #[test]
    fn top_level_keys_parse() {
        let cfg = Config::parse(
            "theme = \"gruvbox-dark\"\nfont = \"Consolas\"\nfont-size = 14\nline-height = 1.4\nscrollback = 5000\ntab-bar = \"auto\"\n",
        )
        .expect_result();
        assert_eq!(cfg.theme, "gruvbox-dark");
        assert_eq!(cfg.font, "Consolas");
        assert_eq!(cfg.font_size, 14.0);
        assert_eq!(cfg.line_height, 1.4);
        assert_eq!(cfg.scrollback, 5000);
        assert_eq!(cfg.tab_bar, TabBar::Auto);
    }

    #[test]
    fn partial_keys_fill_defaults() {
        let cfg = Config::parse("[window]\ncols = 120\n").expect_result();
        assert_eq!(cfg.window.cols, 120);
        assert_eq!(cfg.window.rows, 24);
        assert_eq!(cfg.scrollback, DEFAULT_SCROLLBACK);
    }

    #[test]
    fn font_bundle_matches_top_level_keys() {
        let cfg = Config::parse("font = \"Courier New\"\nfont-size = 9.5\nline-height = 1.1\n")
            .expect_result();
        let font = cfg.font();
        assert_eq!(font.family, "Courier New");
        assert_eq!(font.size, 9.5);
        assert_eq!(font.line_height, 1.1);
    }

    #[test]
    fn tab_bar_values_parse() {
        for (text, want) in [
            ("tab-bar = \"always\"", TabBar::Always),
            ("tab-bar = \"auto\"", TabBar::Auto),
            ("tab-bar = \"hidden\"", TabBar::Hidden),
        ] {
            assert_eq!(Config::parse(text).expect_result().tab_bar, want);
        }
        assert!(Config::parse("tab-bar = \"sideways\"").is_err());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let res = Config::parse("[window]\nbanana = 1\n");
        assert!(res.is_err(), "unknown keys must fail");
        assert!(
            Config::parse("banana = 1").is_err(),
            "unknown top-level keys must fail"
        );
    }

    #[test]
    fn bad_type_is_rejected() {
        let res = Config::parse("[window]\ncols = \"wide\"\n");
        assert!(res.is_err(), "type errors must fail");
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        assert!(
            Config::parse("font-size = 3").is_err(),
            "below MIN_FONT_SIZE"
        );
        assert!(
            Config::parse("font-size = 80").is_err(),
            "above MAX_FONT_SIZE"
        );
        assert!(Config::parse("line-height = 0.1").is_err());
        assert!(Config::parse("line-height = 9").is_err());
        assert!(Config::parse("scrollback = 2000000").is_err());
        assert!(Config::parse("font = \"  \"").is_err(), "blank family");
        assert!(Config::parse("[window]\ncols = 0\n").is_err());
        assert!(Config::parse("[window]\nrows = 5000\n").is_err());
    }

    trait Expect<T> {
        fn expect_result(self) -> T;
    }

    impl<T> Expect<T> for Result<T, anyhow::Error> {
        fn expect_result(self) -> T {
            match self {
                Ok(v) => v,
                Err(e) => panic!("{e}"),
            }
        }
    }
}
