//! Mouse text selection model (WIN-004 copy path).
//!
//! Positions address buffer lines by their monotonic absolute id (stable
//! across scrolling and scrollback eviction) plus a column; a [`Selection`]
//! is the normalized inclusive span between two such positions. All of this
//! is pure math so selection behavior is unit-testable without a window.

/// A grid position: absolute buffer line id plus column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    /// Absolute (monotonic) line id as reported by [`crate::grid::Grid`].
    pub row: usize,
    /// Column index within that line.
    pub col: usize,
}

/// Inclusive span between two grid positions, always normalized so that
/// `start` is at or before `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Earlier end of the span.
    pub start: Pos,
    /// Later end of the span.
    pub end: Pos,
}

impl Selection {
    /// Normalized span covering `a` and `b` (drag order independent).
    #[must_use]
    pub fn between(a: Pos, b: Pos) -> Self {
        let (start, end) = if (a.row, a.col) <= (b.row, b.col) {
            (a, b)
        } else {
            (b, a)
        };
        Self { start, end }
    }

    /// Column range `[lo, hi)` this selection covers on absolute line
    /// `row`, given a grid width of `cols`. `None` when the row is outside
    /// the selection.
    #[must_use]
    pub fn cols_on(&self, row: usize, cols: usize) -> Option<(usize, usize)> {
        if cols == 0 || row < self.start.row || row > self.end.row {
            return None;
        }
        let lo = if row == self.start.row {
            self.start.col.min(cols)
        } else {
            0
        };
        let hi = if row == self.end.row {
            (self.end.col + 1).min(cols)
        } else {
            cols
        };
        Some((lo, hi.max(lo)))
    }

    /// True when `col` on absolute line `row` is inside the selection.
    #[must_use]
    pub fn contains(&self, row: usize, col: usize, cols: usize) -> bool {
        self.cols_on(row, cols)
            .is_some_and(|(lo, hi)| col >= lo && col < hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_is_normalized_regardless_of_drag_direction() {
        let down = Selection::between(Pos { row: 5, col: 3 }, Pos { row: 8, col: 1 });
        let up = Selection::between(Pos { row: 8, col: 1 }, Pos { row: 5, col: 3 });
        assert_eq!(down, up);
        assert_eq!(down.start, Pos { row: 5, col: 3 });
        assert_eq!(down.end, Pos { row: 8, col: 1 });
    }

    #[test]
    fn single_row_span_covers_the_dragged_columns() {
        let sel = Selection::between(Pos { row: 4, col: 2 }, Pos { row: 4, col: 6 });
        assert_eq!(sel.cols_on(4, 10), Some((2, 7)));
        assert_eq!(sel.cols_on(3, 10), None);
        assert_eq!(sel.cols_on(5, 10), None);
    }

    #[test]
    fn multi_row_span_is_block_like_xterm() {
        let sel = Selection::between(Pos { row: 2, col: 5 }, Pos { row: 4, col: 2 });
        assert_eq!(sel.cols_on(2, 8), Some((5, 8)));
        assert_eq!(sel.cols_on(3, 8), Some((0, 8)));
        assert_eq!(sel.cols_on(4, 8), Some((0, 3)));
    }

    #[test]
    fn columns_beyond_a_shrunk_grid_clamp_to_the_width() {
        let sel = Selection::between(Pos { row: 1, col: 40 }, Pos { row: 1, col: 44 });
        assert_eq!(sel.cols_on(1, 20), Some((20, 20)));
    }

    #[test]
    fn contains_matches_the_span_bounds() {
        let sel = Selection::between(Pos { row: 1, col: 1 }, Pos { row: 2, col: 3 });
        assert!(sel.contains(1, 1, 10));
        assert!(!sel.contains(1, 0, 10));
        assert!(sel.contains(2, 3, 10));
        assert!(!sel.contains(2, 4, 10));
    }
}
