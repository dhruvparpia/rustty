//! Theme: UI colors plus the ANSI palette fed to the renderer.
//!
//! The `predawn` preset is the default (spec §14). Themes load from TOML
//! through [`crate::config`].

use crate::color::{Color, ansi_palette};

/// A terminal theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Theme name (preset name or user-provided).
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
    /// ANSI-16 palette (indices 0..=15).
    pub palette: [Color; 16],
}

impl Theme {
    /// The built-in `predawn` preset.
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
            palette: ansi_palette(),
        }
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
}
