//! TOML configuration (spec §14).
//!
//! User file: `%APPDATA%\rusttyterm\config.toml` on Windows. Missing or
//! invalid files fall back to defaults; invalid values are reported by
//! [`Config::load`].

use std::io;
use std::path::PathBuf;

use serde::Deserialize;

/// Window/geometry settings.
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

/// Scrollback settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScrollbackConfig {
    /// Maximum number of lines kept above the viewport (default 1000).
    #[serde(default = "default_scrollback")]
    pub limit: usize,
}

impl Default for ScrollbackConfig {
    fn default() -> Self {
        Self {
            limit: default_scrollback(),
        }
    }
}

/// Theme selection.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeConfig {
    /// Theme preset name (default "predawn").
    #[serde(default = "default_theme")]
    pub name: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: default_theme(),
        }
    }
}

fn default_cols() -> usize {
    80
}

/// Default rows (matches [`Grid::new`]).
fn default_rows() -> usize {
    24
}

/// Default scrollback depth (matches [`Grid::new`]).
fn default_scrollback() -> usize {
    1000
}

fn default_theme() -> String {
    String::from("predawn")
}

/// The full configuration tree.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// `[window]` section.
    #[serde(default)]
    pub window: WindowConfig,
    /// `[scrollback]` section.
    #[serde(default)]
    pub scrollback: ScrollbackConfig,
    /// `[theme]` section.
    #[serde(default)]
    pub theme: ThemeConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            window: WindowConfig {
                cols: default_cols(),
                rows: default_rows(),
            },
            scrollback: ScrollbackConfig {
                limit: default_scrollback(),
            },
            theme: ThemeConfig {
                name: default_theme(),
            },
        }
    }
}

impl Config {
    /// Parse a configuration string.
    ///
    /// # Errors
    ///
    /// Returns an error when the TOML is malformed or a value has the
    /// wrong type.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// Load from the user config file, falling back to defaults.
    ///
    /// A missing file is not an error. A present-but-invalid file is.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read.
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
#[must_use]
pub fn config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("rusttyterm").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_uses_defaults() {
        let cfg = Config::parse("").expect_result();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn partial_sections_fill_defaults() {
        let cfg = Config::parse("[window]\ncols = 120\n").expect_result();
        assert_eq!(cfg.window.cols, 120);
        assert_eq!(cfg.window.rows, 24);
        assert_eq!(cfg.scrollback.limit, 1000);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let res = Config::parse("[window]\nbanana = 1\n");
        assert!(res.is_err(), "unknown keys must fail");
    }

    #[test]
    fn bad_type_is_rejected() {
        let res = Config::parse("[window]\ncols = \"wide\"\n");
        assert!(res.is_err(), "type errors must fail");
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
