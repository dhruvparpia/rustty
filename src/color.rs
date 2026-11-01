//! Color model: 8-bit RGB plus the default ANSI-16 palette.
//!
//! The renderer feeds these into DirectWrite/Direct2D color resources.
//! SGR 16/256/true-color handling lives in the parser (not yet implemented);
//! this module only carries the values.

use std::fmt;

/// An 8-bit RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl Color {
    /// Build a color from 8-bit channels.
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Build a color from a 24-bit hex value (`0xRRGGBB`).
    #[must_use]
    pub const fn from_hex(value: u32) -> Self {
        Self {
            r: ((value >> 16) & 0xFF) as u8,
            g: ((value >> 8) & 0xFF) as u8,
            b: (value & 0xFF) as u8,
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

/// The default ANSI-16 palette used by the `predawn` theme preset
/// (indices 0..=15: 8 normal colors followed by 8 bright colors).
#[must_use]
pub const fn ansi_palette() -> [Color; 16] {
    [
        Color::from_hex(0x000000), // black
        Color::from_hex(0xD7005F), // red
        Color::from_hex(0x00BE6F), // green
        Color::from_hex(0xFF7C36), // yellow
        Color::from_hex(0x28018F), // blue
        Color::from_hex(0xB000AB), // magenta
        Color::from_hex(0x7C70FF), // cyan
        Color::from_hex(0xE7E7E7), // white
        Color::from_hex(0x555555), // bright black
        Color::from_hex(0xF53084), // bright red
        Color::from_hex(0x10CF83), // bright green
        Color::from_hex(0xFFAF4B), // bright yellow
        Color::from_hex(0x5442A3), // bright blue
        Color::from_hex(0xAE21A9), // bright magenta
        Color::from_hex(0x66A6E3), // bright cyan
        Color::from_hex(0xFFFFFF), // bright white
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_hex_splits_channels() {
        let c = Color::from_hex(0x12_34_56);
        assert_eq!(c, Color::rgb(0x12, 0x34, 0x56));
    }

    #[test]
    fn display_is_hash_hex() {
        assert_eq!(Color::rgb(0xFF, 0x00, 0x7C).to_string(), "#FF007C");
    }

    #[test]
    fn palette_has_16_colors() {
        assert_eq!(ansi_palette()[0], Color::rgb(0, 0, 0));
        assert_eq!(ansi_palette()[15], Color::rgb(0xFF, 0xFF, 0xFF));
    }
}
