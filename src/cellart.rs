//! Characters drawn as geometry rather than font glyphs: Block Elements
//! (U+2580-259F) and the Powerline separators (U+E0B0-E0B3).
//!
//! A font's block glyph stops at the font's own line box, so any line height
//! above 1.0 leaves gaps between rows. Filling the cell instead makes rows join
//! exactly. Pure math: the renderer draws what this module returns, and nothing
//! here allocates, because it runs in the render hot path.

/// A rectangle in cell coordinates: 0..1 across and 0..1 down the cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect01 {
    /// Left edge.
    pub x0: f32,
    /// Top edge.
    pub y0: f32,
    /// Right edge.
    pub x1: f32,
    /// Bottom edge.
    pub y1: f32,
}

/// A Powerline separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerlineSide {
    /// U+E0B0: solid triangle pointing right.
    SolidRight,
    /// U+E0B1: thin chevron pointing right.
    ThinRight,
    /// U+E0B2: solid triangle pointing left.
    SolidLeft,
    /// U+E0B3: thin chevron pointing left.
    ThinLeft,
}

/// How to draw a character without the font.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// Fill the first `count` of `rects` with the foreground, blended over
    /// the background by `alpha` (shade characters use less than 1).
    Fill {
        /// Rectangles to fill.
        rects: [Rect01; 4],
        /// How many of `rects` are in use.
        count: u8,
        /// Foreground strength.
        alpha: f32,
    },
    /// A Powerline separator covering the cell.
    Triangle(PowerlineSide),
}

const EMPTY: Rect01 = rect(0.0, 0.0, 0.0, 0.0);

const fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect01 {
    Rect01 { x0, y0, x1, y1 }
}

const fn one(r: Rect01) -> Shape {
    Shape::Fill {
        rects: [r, EMPTY, EMPTY, EMPTY],
        count: 1,
        alpha: 1.0,
    }
}

const fn shade(alpha: f32) -> Shape {
    Shape::Fill {
        rects: [rect(0.0, 0.0, 1.0, 1.0), EMPTY, EMPTY, EMPTY],
        count: 1,
        alpha,
    }
}

const fn quads(a: Rect01, b: Rect01, c: Rect01, count: u8) -> Shape {
    Shape::Fill {
        rects: [a, b, c, EMPTY],
        count,
        alpha: 1.0,
    }
}

// Quadrants, named as in the Unicode character names.
const UL: Rect01 = rect(0.0, 0.0, 0.5, 0.5);
const UR: Rect01 = rect(0.5, 0.0, 1.0, 0.5);
const LL: Rect01 = rect(0.0, 0.5, 0.5, 1.0);
const LR: Rect01 = rect(0.5, 0.5, 1.0, 1.0);

/// `n` eighths of a cell.
fn eighths(n: u32) -> f32 {
    n as f32 / 8.0
}

/// The geometry for `ch`, or `None` when the font should draw it.
#[must_use]
pub fn shape(ch: char) -> Option<Shape> {
    let cp = u32::from(ch);
    Some(match cp {
        0x2580 => one(rect(0.0, 0.0, 1.0, 0.5)),
        // Lower one eighth (U+2581) up to lower seven eighths (U+2587).
        0x2581..=0x2587 => one(rect(0.0, 1.0 - eighths(cp - 0x2580), 1.0, 1.0)),
        0x2588 => one(rect(0.0, 0.0, 1.0, 1.0)),
        // Left seven eighths (U+2589) down to left one eighth (U+258F).
        0x2589..=0x258F => one(rect(0.0, 0.0, eighths(0x2590 - cp), 1.0)),
        0x2590 => one(rect(0.5, 0.0, 1.0, 1.0)),
        0x2591 => shade(0.25),
        0x2592 => shade(0.5),
        0x2593 => shade(0.75),
        0x2594 => one(rect(0.0, 0.0, 1.0, 0.125)),
        0x2595 => one(rect(0.875, 0.0, 1.0, 1.0)),
        0x2596 => one(LL),
        0x2597 => one(LR),
        0x2598 => one(UL),
        0x2599 => quads(UL, LL, LR, 3),
        0x259A => quads(UL, LR, EMPTY, 2),
        0x259B => quads(UL, UR, LL, 3),
        0x259C => quads(UL, UR, LR, 3),
        0x259D => one(UR),
        0x259E => quads(UR, LL, EMPTY, 2),
        0x259F => quads(UR, LL, LR, 3),
        0xE0B0 => Shape::Triangle(PowerlineSide::SolidRight),
        0xE0B1 => Shape::Triangle(PowerlineSide::ThinRight),
        0xE0B2 => Shape::Triangle(PowerlineSide::SolidLeft),
        0xE0B3 => Shape::Triangle(PowerlineSide::ThinLeft),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(ch: char) -> (Vec<Rect01>, f32) {
        match shape(ch) {
            Some(Shape::Fill {
                rects,
                count,
                alpha,
            }) => (rects[..usize::from(count)].to_vec(), alpha),
            other => panic!("{ch:?}: expected a fill, got {other:?}"),
        }
    }

    fn r(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect01 {
        Rect01 { x0, y0, x1, y1 }
    }

    #[test]
    fn upper_half_block_fills_the_top_half() {
        // Break caught: the crab's main character filling the wrong half.
        assert_eq!(fill('▀'), (vec![r(0.0, 0.0, 1.0, 0.5)], 1.0));
    }

    #[test]
    fn lower_half_and_full_blocks() {
        assert_eq!(fill('▄'), (vec![r(0.0, 0.5, 1.0, 1.0)], 1.0));
        assert_eq!(fill('█'), (vec![r(0.0, 0.0, 1.0, 1.0)], 1.0));
    }

    #[test]
    fn quadrants_cover_the_right_corners() {
        assert_eq!(fill('▘'), (vec![r(0.0, 0.0, 0.5, 0.5)], 1.0));
        let (rects, _) = fill('▟');
        assert_eq!(
            rects,
            vec![
                r(0.5, 0.0, 1.0, 0.5),
                r(0.0, 0.5, 0.5, 1.0),
                r(0.5, 0.5, 1.0, 1.0)
            ]
        );
        let (diagonal, _) = fill('▚');
        assert_eq!(diagonal, vec![r(0.0, 0.0, 0.5, 0.5), r(0.5, 0.5, 1.0, 1.0)]);
    }

    #[test]
    fn eighths_and_shades() {
        assert_eq!(fill('▁').0, vec![r(0.0, 0.875, 1.0, 1.0)]);
        assert_eq!(fill('▇').0, vec![r(0.0, 0.125, 1.0, 1.0)]);
        assert_eq!(fill('▏').0, vec![r(0.0, 0.0, 0.125, 1.0)]);
        assert_eq!(fill('▉').0, vec![r(0.0, 0.0, 0.875, 1.0)]);
        assert_eq!(fill('▔').0, vec![r(0.0, 0.0, 1.0, 0.125)]);
        assert_eq!(fill('▕').0, vec![r(0.875, 0.0, 1.0, 1.0)]);
        assert_eq!(fill('▐').0, vec![r(0.5, 0.0, 1.0, 1.0)]);
        assert_eq!(fill('░').1, 0.25);
        assert_eq!(fill('▒').1, 0.5);
        assert_eq!(fill('▓').1, 0.75);
    }

    #[test]
    fn every_block_element_has_a_shape() {
        for cp in 0x2580..=0x259F_u32 {
            let ch = char::from_u32(cp).expect("valid char");
            assert!(shape(ch).is_some(), "U+{cp:04X}");
        }
    }

    #[test]
    fn powerline_separators_are_triangles() {
        for (ch, side) in [
            ('\u{E0B0}', PowerlineSide::SolidRight),
            ('\u{E0B1}', PowerlineSide::ThinRight),
            ('\u{E0B2}', PowerlineSide::SolidLeft),
            ('\u{E0B3}', PowerlineSide::ThinLeft),
        ] {
            assert_eq!(shape(ch), Some(Shape::Triangle(side)));
        }
    }

    #[test]
    fn ordinary_text_keeps_the_font() {
        for ch in ['a', ' ', '─', '\u{E0A0}', '\u{E0B4}', '中'] {
            assert_eq!(shape(ch), None, "{ch:?}");
        }
    }
}
