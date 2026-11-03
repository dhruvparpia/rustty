//! Core screen buffer: circular line buffer, viewport, resize, damage tracking.
//!
//! Grid semantics (spec §"Grid semantics", MVP):
//! - One screen buffer per tab/pane: a circular buffer of lines, capacity = rows + scrollback.
//! - The viewport is the last `rows` lines of the buffer; `scroll_offset` shifts it backwards.
//! - Writes target viewport-relative rows; appends grow the buffer and evict the oldest line when full.
//! - Resize preserves content (keeping the newest lines), clamps scroll, and repaints everything once.
//! - Writes when scrolled back do not move the viewport.

use crate::color::Color;
use crate::selection::Selection;

/// A single character cell in the grid
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The character displayed in this cell (space for blank)
    pub char: char,
    /// Foreground color; `None` uses the theme's default foreground
    pub fg: Option<Color>,
    /// Background color; `None` uses the theme's default background
    pub bg: Option<Color>,
    /// Active SGR attributes
    pub attributes: CellAttribute,
    /// OSC 8 hyperlink id into the parser's URI table; `0` means no link.
    /// The id travels with the cell through scrollback, selection and
    /// resize for free (issue #41).
    pub link: u32,
}

impl Cell {
    /// Create a blank cell (space, theme defaults, no attributes)
    pub fn blank() -> Self {
        Self {
            char: ' ',
            fg: None,
            bg: None,
            attributes: CellAttribute::empty(),
            link: 0,
        }
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::blank()
    }
}

/// SGR attribute flags for a cell
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellAttribute(u8);

impl CellAttribute {
    /// Bold (SGR 1)
    pub const BOLD: Self = Self(1 << 0);
    /// Dim / faint (SGR 2)
    pub const DIM: Self = Self(1 << 1);
    /// Italic (SGR 3)
    pub const ITALIC: Self = Self(1 << 2);
    /// Underline (SGR 4)
    pub const UNDERLINE: Self = Self(1 << 3);
    /// Blink (SGR 5)
    pub const BLINK: Self = Self(1 << 4);
    /// Inverse / reverse video (SGR 7)
    pub const INVERSE: Self = Self(1 << 5);
    /// Hidden (SGR 8)
    pub const HIDDEN: Self = Self(1 << 6);
    /// Crossed out (SGR 9)
    pub const STRIKE: Self = Self(1 << 7);

    /// Empty attribute set (plain text)
    pub const fn empty() -> Self {
        Self(0)
    }

    /// All attribute flags
    pub const fn all() -> Self {
        Self(0xFF)
    }

    /// Raw flag bits (used by the SGR parser and renderer pen construction)
    #[allow(dead_code, reason = "needed by the parser/renderer milestones")]
    pub const fn flags(self) -> u8 {
        self.0
    }

    /// Union of two attribute sets
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Intersection of two attribute sets
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Remove `other`'s flags from this set (used by SGR resets, e.g. 22 clears
    /// bold and dim at once)
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// True if every flag in `other` is set in `self`
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl std::ops::BitOr for CellAttribute {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self::union(self, rhs)
    }
}

impl std::ops::BitAnd for CellAttribute {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self {
        Self::intersection(self, rhs)
    }
}

/// A line in the screen buffer
#[derive(Debug, Clone, Default)]
pub struct Line {
    /// Cells making up this line
    pub cells: Vec<Cell>,
    /// Damage flag, cleared by [`Grid::take_dirty_rows`]
    pub is_dirty: bool,
}

impl Line {
    /// Create a line of `cols` blank cells
    pub fn blank(cols: usize) -> Self {
        Self {
            cells: vec![Cell::blank(); cols],
            is_dirty: false,
        }
    }
}

/// Circular screen buffer with a scrolling viewport.
///
/// The buffer holds at most `rows + scrollback` lines. The virtual line
/// stream is addressed from oldest (`0`) to newest (`total - 1`); physical
/// slots are `(head + virtual) % capacity`. The viewport shows `rows`
/// consecutive lines starting at `total - rows - scroll_offset`.
#[derive(Debug, Clone)]
pub struct Grid {
    /// Number of columns
    pub cols: usize,
    /// Number of visible rows (viewport height)
    pub rows: usize,
    /// Number of lines kept above the viewport
    pub scrollback: usize,
    lines: Vec<Line>,
    head: usize,
    total: usize,
    /// Lines evicted from the front of the buffer. Absolute line ids are
    /// `evicted + virtual_index`, so they only ever grow and stay valid
    /// for scrollback consumers (selection).
    evicted: usize,
    scroll_offset: usize,
    full_dirty: bool,
}

impl Grid {
    /// Create a new grid with custom dimensions and scrollback
    pub fn with_size(cols: usize, rows: usize, scrollback: usize) -> Self {
        let rows = rows.max(1);
        let capacity = rows + scrollback;
        // Only the viewport rows are materialised. Slots at virtual index
        // >= total are unreachable (every accessor gates on `total`) and
        // stay empty until a write fills them, so a big scrollback costs
        // one Vec of small `Line` structs, not one cell Vec per slot
        // (issue #15).
        let mut lines = Vec::with_capacity(capacity);
        lines.resize(rows, Line::blank(cols));
        lines.resize(capacity, Line::default());
        Self {
            cols,
            rows,
            scrollback,
            lines,
            head: 0,
            total: rows,
            evicted: 0,
            scroll_offset: 0,
            full_dirty: true,
        }
    }

    /// Create a grid with default 80x24 dimensions and 1000-line scrollback
    pub fn new() -> Self {
        Self::with_size(80, 24, 1000)
    }

    /// Total number of lines currently held in the buffer
    pub fn total_lines(&self) -> usize {
        self.total
    }

    /// Current scroll offset in lines above the viewport
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Number of lines of scrollback retained above the viewport
    pub fn scrollback(&self) -> usize {
        self.scrollback
    }

    /// Buffer capacity: `rows + scrollback` lines
    pub fn capacity(&self) -> usize {
        self.lines.len()
    }

    /// Viewport width in columns
    pub const fn cols(&self) -> usize {
        self.cols
    }

    /// Viewport height in rows
    pub const fn rows(&self) -> usize {
        self.rows
    }

    /// Virtual index of the viewport's first line
    fn top(&self) -> usize {
        self.total.saturating_sub(self.rows + self.scroll_offset)
    }

    /// Top virtual index of the *live* (unscrolled) viewport. Parser
    /// writes and scroll operations resolve against this (issue #2): with
    /// the user scrolled back, output lands on the live screen, never in
    /// the scrollback the viewport happens to be showing.
    fn live_top(&self) -> usize {
        self.total.saturating_sub(self.rows)
    }

    /// Live-screen row (not the scrolled viewport row).
    fn live_row(&self, row: usize) -> Option<&Line> {
        let vi = self.live_top() + row;
        if vi >= self.total {
            return None;
        }
        Some(&self.lines[self.slot(vi)])
    }

    /// Physical slot for a virtual line index
    fn slot(&self, virtual_index: usize) -> usize {
        (self.head + virtual_index) % self.capacity()
    }

    /// Clamp the scroll offset so the viewport stays on the buffer
    fn clamp_scroll(&mut self) {
        self.scroll_offset = self.scroll_offset.min(self.total.saturating_sub(self.rows));
    }

    /// Append a line at the end of the buffer, evicting the oldest when full
    pub fn append_line(&mut self, line: Line) {
        let old_top = self.top();
        let evicting = self.total >= self.capacity();
        let idx = if !evicting {
            let idx = (self.head + self.total) % self.capacity();
            self.total += 1;
            idx
        } else {
            let idx = self.head;
            self.head = (self.head + 1) % self.capacity();
            self.evicted += 1;
            idx
        };
        self.lines[idx] = line;
        self.lines[idx].is_dirty = true;
        // Issue #13: eviction (or the buffer top crossing the viewport top)
        // shifts every visible row onto the next buffer line, so the new
        // line's own dirty flag is not enough. Mark the viewport fully
        // dirty; writers must never clear a pending full-redraw event.
        if evicting || self.top() != old_top {
            self.full_dirty = true;
        }
    }

    /// Write a line to a viewport row (0-based). Rows past the end of the
    /// buffer pad with blank lines; the viewport never moves on a write.
    pub fn write_line(&mut self, row: usize, line: Line) {
        let row = row.min(self.rows.saturating_sub(1));
        let virtual_index = self.live_top() + row;
        while self.total <= virtual_index {
            self.append_line(Line::blank(self.cols));
        }
        let slot = self.slot(virtual_index);
        self.lines[slot] = line;
        self.lines[slot].is_dirty = true;
    }

    /// Write a stub line containing plain text (helper for tests and scaffolding)
    pub fn write_line_stub(&mut self, row: usize, content: &str) {
        let mut line = Line::blank(self.cols);
        for (i, ch) in content.chars().take(self.cols).enumerate() {
            line.cells[i].char = ch;
        }
        self.write_line(row, line);
    }

    /// Write a cell to a viewport position
    pub fn write_cell(&mut self, row: usize, col: usize, cell: Cell) {
        let row = row.min(self.rows.saturating_sub(1));
        let virtual_index = self.live_top() + row;
        while self.total <= virtual_index {
            self.append_line(Line::blank(self.cols));
        }
        let slot = self.slot(virtual_index);
        if let Some(dest) = self.lines[slot].cells.get_mut(col) {
            *dest = cell;
        }
        self.lines[slot].is_dirty = true;
    }

    /// Get a reference to a viewport line
    pub fn row(&self, row: usize) -> Option<&Line> {
        if row >= self.rows {
            return None;
        }
        let virtual_index = self.top() + row;
        if virtual_index >= self.total {
            return None;
        }
        Some(&self.lines[self.slot(virtual_index)])
    }

    /// Text content of a viewport line, with trailing blanks trimmed.
    ///
    /// Handy for tests and renderer debugging; `None` when `row` is outside
    /// the viewport or not backed by a line.
    #[must_use]
    pub fn line_text(&self, row: usize) -> Option<String> {
        Some(
            self.row(row)?
                .cells
                .iter()
                .map(|c| c.char)
                .collect::<String>()
                .trim_end()
                .to_string(),
        )
    }

    /// Absolute line id of live-screen row `row` (unscrolled viewport),
    /// `None` when the row has no backing line. Used by command/bookmark
    /// anchors so reading scrollback never moves them (issue #78).
    #[must_use]
    pub fn live_abs(&self, row: usize) -> Option<usize> {
        let v = self.live_top() + row;
        if v >= self.total {
            return None;
        }
        Some(self.evicted + v)
    }

    /// Absolute (monotonic) line id of viewport row `row`, `None` when
    /// the row has no backing line.
    #[must_use]
    pub fn abs_of_view(&self, row: usize) -> Option<usize> {
        if row >= self.rows {
            return None;
        }
        let v = self.top() + row;
        if v >= self.total {
            return None;
        }
        Some(self.evicted + v)
    }

    /// Viewport row that shows absolute line `abs`, `None` when the line
    /// is scrolled off-screen or already evicted.
    #[must_use]
    pub fn view_of_abs(&self, abs: usize) -> Option<usize> {
        let v = abs.checked_sub(self.evicted)?;
        let top = self.top();
        if v >= self.total || v < top || v >= top + self.rows {
            return None;
        }
        Some(v - top)
    }

    /// Oldest absolute line id still held in the buffer.
    #[must_use]
    pub fn first_abs(&self) -> usize {
        self.evicted
    }

    /// Newest absolute line id held in the buffer.
    #[must_use]
    pub fn last_abs(&self) -> usize {
        self.evicted + self.total.saturating_sub(1)
    }

    /// Reference to the line with absolute id `abs`, `None` once evicted.
    #[must_use]
    pub fn line_abs(&self, abs: usize) -> Option<&Line> {
        let v = abs.checked_sub(self.evicted)?;
        if v >= self.total {
            return None;
        }
        Some(&self.lines[self.slot(v)])
    }

    /// Trimmed text of absolute line `abs`, `None` once evicted.
    #[must_use]
    pub fn line_text_abs(&self, abs: usize) -> Option<String> {
        Some(
            self.line_abs(abs)?
                .cells
                .iter()
                .map(|c| c.char)
                .collect::<String>()
                .trim_end()
                .to_string(),
        )
    }

    /// Word bounds `[lo, hi)` around column `col` of absolute line `abs`
    /// (double-click selection). Word characters are alphanumerics plus
    /// `_ - . / \ : @`; a blank column selects itself.
    #[must_use]
    pub fn word_range_abs(&self, abs: usize, col: usize) -> Option<(usize, usize)> {
        let line = self.line_abs(abs)?;
        let len = line.cells.len();
        let col = col.min(len.saturating_sub(1));
        let in_word =
            |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '\\' | ':' | '@');
        if !in_word(line.cells.get(col)?.char) {
            return Some((col, col + 1));
        }
        let mut lo = col;
        while lo > 0 && line.cells[lo - 1].char != ' ' && in_word(line.cells[lo - 1].char) {
            lo -= 1;
        }
        let mut hi = col + 1;
        while hi < len && line.cells[hi].char != ' ' && in_word(line.cells[hi].char) {
            hi += 1;
        }
        Some((lo, hi))
    }

    /// Plain-text copy of `sel`: rows joined with `\n`, each row's trailing
    /// blanks trimmed, trailing empty lines dropped. Rows evicted from the
    /// scrollback are skipped; a fully evicted selection yields "".
    #[must_use]
    pub fn selection_text(&self, sel: &Selection) -> String {
        let start = sel.start.row.max(self.first_abs());
        let end = sel.end.row.min(self.last_abs());
        let mut out = String::new();
        for abs in start..=end {
            let Some((lo, hi)) = sel.cols_on(abs, self.cols) else {
                continue;
            };
            let Some(line) = self.line_abs(abs) else {
                continue;
            };
            let hi = hi.min(line.cells.len());
            let text: String = line.cells[lo.min(hi)..hi].iter().map(|c| c.char).collect();
            out.push_str(text.trim_end());
            out.push('\n');
        }
        while out.ends_with('\n') {
            out.pop();
        }
        out
    }

    /// Get a mutable reference to a viewport line, marking it dirty
    pub fn row_mut(&mut self, row: usize) -> Option<&mut Line> {
        if row >= self.rows {
            return None;
        }
        let virtual_index = self.top() + row;
        if virtual_index >= self.total {
            return None;
        }
        let slot = self.slot(virtual_index);
        self.lines[slot].is_dirty = true;
        Some(&mut self.lines[slot])
    }

    /// Resize the viewport and reallocate the buffer.
    ///
    /// Content is preserved by keeping the newest lines (the oldest are
    /// evicted when the capacity shrinks, the newly available slots on
    /// growth come back blank). Scroll is clamped and every row is marked
    /// dirty so the next frame repaints once.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);

        // Drain the live lines in virtual order.

        let mut content = Vec::with_capacity(self.total);
        for v in 0..self.total {
            let slot = self.slot(v);
            content.push(std::mem::take(&mut self.lines[slot]));
        }

        let rows = rows.min(rows + self.scrollback);
        let new_capacity = rows + self.scrollback;
        let keep = content.len().min(new_capacity);
        let start = content.len() - keep;

        let mut lines = Vec::with_capacity(new_capacity);
        // Every retained line must match the new width: pad truncated lines
        // with blanks, trim widened ones. Skipping this lets a line keep a
        // shorter cell vector than `self.cols`, and column-indexed writes
        // then run out of bounds.
        lines.extend(content.into_iter().skip(start).map(|mut line| {
            let mut cells = std::mem::take(&mut line.cells);
            cells.resize(cols, Cell::blank());
            line.cells = cells;
            line.is_dirty = true;
            line
        }));

        self.cols = cols;
        self.rows = rows;
        // The viewport must always have `rows` backing lines: when the
        // buffer holds fewer lines than the new viewport (a fresh grow),
        // the extra bottom slots come back blank. Slots beyond `total`
        // stay empty: they are unreadable, and materialising them is the
        // O(capacity) resize cost this issue measured (issue #15).
        self.total = keep.max(rows);
        lines.resize(self.total, Line::blank(cols));
        lines.resize(new_capacity, Line::default());
        self.lines = lines;
        self.head = 0;
        // The lines we dropped are gone forever: keep absolute ids monotonic.
        self.evicted += start;
        self.clamp_scroll();
        self.mark_all_dirty();
    }

    /// Change the scrollback depth and re-allocate the buffer
    pub fn set_scrollback(&mut self, scrollback: usize) {
        self.scrollback = scrollback;
        let (cols, rows) = (self.cols, self.rows);
        self.resize(cols, rows);
    }

    /// Scroll the viewport up by `n` lines (into history)
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
        self.clamp_scroll();
        self.mark_all_dirty();
    }

    /// Scroll the viewport down by `n` lines (towards the live edge)
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        self.clamp_scroll();
        self.mark_all_dirty();
    }

    /// Bring absolute line `abs` into the viewport, at its first visible
    /// row. `false` when the line has been evicted. Used by search navigation
    /// (SEARCH-005/006) so a match in scrollback is visible.
    pub fn scroll_to_abs(&mut self, abs: usize) -> bool {
        let Some(v) = abs.checked_sub(self.evicted) else {
            return false;
        };
        if v >= self.total {
            return false;
        }
        let top = self.top();
        if v >= top && v < top + self.rows {
            return true;
        }
        let max_offset = self.total.saturating_sub(self.rows);
        self.scroll_offset = max_offset.saturating_sub(v);
        self.mark_all_dirty();
        true
    }

    /// Fill viewport rows `top..=bottom` with blank lines (ED / EL / ECH use this)
    pub fn erase_rows(&mut self, top: usize, bottom: usize) {
        let top = top.min(self.rows.saturating_sub(1));
        let bottom = bottom.min(self.rows.saturating_sub(1));
        for row in top..=bottom {
            self.write_line(row, Line::blank(self.cols));
        }
    }

    /// Drop every scrollback line, keeping the live viewport's content
    /// (issue #7: ED 3 / "clear history" must never clear the visible
    /// screen; only a scrollback-reading TUI's history is lost).
    pub fn clear_scrollback(&mut self) {
        let live: Vec<Line> = (0..self.rows)
            .filter_map(|r| self.live_row(r).cloned())
            .collect();
        let (cols, rows, sb) = (self.cols, self.rows, self.scrollback);
        let start_abs = self.evicted + self.live_top();
        *self = Grid::with_size(cols, rows, sb);
        for (i, line) in live.into_iter().enumerate() {
            self.write_line(i, line);
        }
        self.evicted = start_abs;
        self.mark_all_dirty();
    }
    /// Shift viewport rows `top..=bottom` up by one line; the region's bottom
    /// row becomes blank. Used by DL, ESC M and SU on partial scroll regions.
    ///
    /// On the full viewport the content leaving the top goes into scrollback,
    /// so this appends a blank line at the live edge instead of cloning rows.
    pub fn scroll_region_up(&mut self, top: usize, bottom: usize) {
        let bottom = bottom.min(self.rows.saturating_sub(1));
        if top > bottom {
            return;
        }
        if top == 0 && bottom == self.rows - 1 {
            self.append_line(Line::blank(self.cols));
            return;
        }
        for row in top..bottom {
            let next = self
                .live_row(row + 1)
                .cloned()
                .unwrap_or_else(|| Line::blank(self.cols));
            self.write_line(row, next);
        }
        self.write_line(bottom, Line::blank(self.cols));
    }

    /// Shift viewport rows `top..=bottom` down by one line; the region's top
    /// row becomes blank and the bottom row of the region is discarded.
    /// Used by IL and SD.
    pub fn scroll_region_down(&mut self, top: usize, bottom: usize) {
        let top = top.min(self.rows.saturating_sub(1));
        let bottom = bottom.min(self.rows.saturating_sub(1));
        if top > bottom {
            return;
        }
        for row in (top + 1..=bottom).rev() {
            let prev = self
                .live_row(row - 1)
                .cloned()
                .unwrap_or_else(|| Line::blank(self.cols));
            self.write_line(row, prev);
        }
        self.write_line(top, Line::blank(self.cols));
    }

    /// Mark every row in the viewport dirty (full repaint)
    pub fn mark_all_dirty(&mut self) {
        self.full_dirty = true;
    }

    /// Mark a single viewport row dirty
    pub fn mark_dirty(&mut self, row: usize) {
        if row < self.rows {
            let virtual_index = self.top() + row;
            if virtual_index < self.total {
                let slot = self.slot(virtual_index);
                self.lines[slot].is_dirty = true;
            }
        }
    }

    /// Viewport rows that need repainting.
    ///
    /// Returns all rows after a full-redraw event (resize, scroll),
    /// otherwise only the rows written to since the last take.
    pub fn dirty_rows(&self) -> Vec<usize> {
        if self.full_dirty {
            return (0..self.rows).collect();
        }
        let top = self.top();
        (0..self.rows)
            .filter(|&row| {
                let virtual_index = top + row;
                virtual_index < self.total && self.lines[self.slot(virtual_index)].is_dirty
            })
            .collect()
    }

    /// `true` when a full-redraw event (resize, scroll, erase) is pending,
    /// i.e. every viewport row needs repainting.
    #[must_use]
    pub fn all_dirty(&self) -> bool {
        self.full_dirty
    }

    /// Report every viewport row needing repaint, in ascending order,
    /// without allocating (NFR-113 hot path: the renderer keeps its own
    /// row mask and calls this once per frame).
    pub fn for_each_dirty_row(&self, mut f: impl FnMut(usize)) {
        if self.full_dirty {
            for row in 0..self.rows {
                f(row);
            }
            return;
        }
        let top = self.top();
        for row in 0..self.rows {
            let virtual_index = top + row;
            if virtual_index < self.total && self.lines[self.slot(virtual_index)].is_dirty {
                f(row);
            }
        }
    }

    /// Clear every damage flag. Call after the dirty rows have been painted;
    /// rows that were not painted must be marked dirty again by the caller.
    pub fn clear_dirty_rows(&mut self) {
        if self.full_dirty {
            for line in &mut self.lines {
                line.is_dirty = false;
            }
            self.full_dirty = false;
            return;
        }
        let top = self.top();
        for row in 0..self.rows {
            let virtual_index = top + row;
            if virtual_index < self.total {
                let slot = self.slot(virtual_index);
                self.lines[slot].is_dirty = false;
            }
        }
    }

    /// Collect the dirty rows and clear their damage flags
    pub fn take_dirty_rows(&mut self) -> Vec<usize> {
        let rows = self.dirty_rows();
        if self.full_dirty {
            for line in &mut self.lines {
                line.is_dirty = false;
            }
            self.full_dirty = false;
        } else {
            let top = self.top();
            for row in &rows {
                let virtual_index = top + row;
                if virtual_index < self.total {
                    let slot = self.slot(virtual_index);
                    self.lines[slot].is_dirty = false;
                }
            }
        }
        rows
    }
}

impl Default for Grid {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::Pos;

    fn make_line(content: &str, cols: usize) -> Line {
        let mut line = Line::blank(cols);
        for (i, ch) in content.chars().take(cols).enumerate() {
            line.cells[i].char = ch;
        }
        line
    }

    /// Simulate a terminal printing `n` distinct lines (append at the live edge)
    fn append_lines(grid: &mut Grid, n: usize) {
        for i in 1..=n {
            grid.append_line(make_line(&format!("line {i}"), grid.cols));
        }
    }

    fn row_text(grid: &Grid, row: usize) -> String {
        grid.line_text(row).unwrap()
    }

    #[test]
    fn test_grid_initialization() {
        let grid = Grid::new();
        assert_eq!(grid.cols, 80);
        assert_eq!(grid.rows, 24);
        assert_eq!(grid.scrollback(), 1000);
        assert_eq!(grid.capacity(), 1024);
        assert_eq!(grid.total_lines(), 24);
        assert_eq!(grid.scroll_offset(), 0);

        let grid = Grid::with_size(80, 24, 100);
        assert_eq!(grid.cols, 80);
        assert_eq!(grid.rows, 24);
        assert_eq!(grid.scrollback(), 100);
        assert_eq!(grid.capacity(), 124);
    }

    #[test]
    fn test_initial_frame_is_dirty() {
        let grid = Grid::with_size(80, 24, 100);
        assert_eq!(grid.dirty_rows(), (0..24).collect::<Vec<_>>());
    }

    #[test]
    fn test_append_and_viewport() {
        let mut grid = Grid::with_size(80, 24, 100);
        // The viewport starts as `rows` blank lines; appending pushes them up.
        append_lines(&mut grid, 50);
        assert_eq!(grid.total_lines(), 24 + 50);
        assert_eq!(row_text(&grid, 23), "line 50");
        assert_eq!(row_text(&grid, 0), "line 27");
    }

    #[test]
    fn test_eviction_when_buffer_full() {
        let mut grid = Grid::with_size(80, 24, 100);
        append_lines(&mut grid, 124);
        assert_eq!(grid.total_lines(), 124);
        assert_eq!(row_text(&grid, 0), "line 101");
        assert_eq!(row_text(&grid, 23), "line 124");
        // One more line evicts the oldest (line 1); oldest kept is line 2.
        append_lines(&mut grid, 1);
        assert_eq!(grid.total_lines(), 124);
    }

    #[test]
    fn test_eviction_discards_oldest_line() {
        let mut grid = Grid::with_size(80, 24, 100);
        append_lines(&mut grid, 125);
        assert_eq!(grid.total_lines(), 124);
        // Scrolled fully to the top, the first retained line is "line 2".
        grid.scroll_up(100);
        assert_eq!(grid.scroll_offset(), 100);
        assert_eq!(row_text(&grid, 0), "line 2");
    }

    #[test]
    fn test_scrolling() {
        let mut grid = Grid::with_size(80, 24, 100);
        append_lines(&mut grid, 50);
        // total 74, top at live edge = 50
        grid.scroll_up(20);
        assert_eq!(grid.scroll_offset(), 20);
        assert_eq!(row_text(&grid, 0), "line 7");
        grid.scroll_down(10);
        assert_eq!(grid.scroll_offset(), 10);
        grid.scroll_down(999);
        assert_eq!(grid.scroll_offset(), 0);
        assert_eq!(row_text(&grid, 0), "line 27");
    }

    #[test]
    fn test_scroll_to_abs_brings_a_line_to_the_top() {
        let mut grid = Grid::with_size(80, 24, 100);
        append_lines(&mut grid, 50);
        // Absolute id 30 is "line 7"; it is not visible at the live edge.
        assert!(grid.scroll_to_abs(30));
        assert_eq!(grid.scroll_offset(), 20);
        assert_eq!(row_text(&grid, 0), "line 7");
        assert!(grid.scroll_to_abs(50), "line 27 is already visible");
        assert_eq!(grid.scroll_offset(), 20);
        assert!(!grid.scroll_to_abs(999), "evicted id is not reachable");
    }

    #[test]
    fn test_write_while_scrolled_back_does_not_move_viewport() {
        let mut grid = Grid::with_size(80, 24, 100);
        append_lines(&mut grid, 50);
        grid.scroll_up(10);
        grid.write_line(23, make_line("typed", grid.cols));
        // Issue #2: the write lands on the live screen, not the history
        // row the viewport is showing, and the viewport stays put.
        assert_eq!(grid.scroll_offset(), 10);
        grid.scroll_down(10);
        assert_eq!(row_text(&grid, 23), "typed");
        assert_eq!(grid.total_lines(), 74);
    }

    #[test]
    fn test_resize_preserves_newest_lines() {
        let mut grid = Grid::with_size(80, 24, 100);
        append_lines(&mut grid, 200);
        // Buffer full at capacity 124; shrink rows to 10 -> capacity 110
        grid.resize(80, 10);
        assert_eq!(grid.rows, 10);
        assert_eq!(grid.capacity(), 110);
        assert_eq!(grid.total_lines(), 110);
        assert_eq!(row_text(&grid, 9), "line 200");
        // Grow back: viewport shows the last 24 of the 110 retained lines
        grid.resize(80, 24);
        assert_eq!(grid.rows, 24);
        assert_eq!(grid.total_lines(), 110);
        assert_eq!(row_text(&grid, 23), "line 200");
        assert_eq!(row_text(&grid, 0), "line 177");
    }

    #[test]
    fn test_resize_marks_all_dirty() {
        let mut grid = Grid::with_size(80, 24, 100);
        grid.clear_dirty_rows(); // simulate the initial full repaint
        let mut line = Line::blank(80);
        line.cells[0].char = 'x';
        grid.write_line(5, line);
        assert_eq!(grid.take_dirty_rows(), vec![5]);
        assert!(grid.dirty_rows().is_empty());
        grid.resize(100, 30);
        assert_eq!(grid.dirty_rows(), (0..30).collect::<Vec<_>>());
    }

    #[test]
    fn test_resize_clamps_scroll() {
        let mut grid = Grid::with_size(80, 24, 100);
        append_lines(&mut grid, 50);
        grid.scroll_up(40);
        grid.resize(80, 10);
        // total is 60 now, max offset = 60 - 10 = 50; 40 fits, viewport at top
        assert_eq!(grid.scroll_offset(), 40);
        assert_eq!(row_text(&grid, 0), "line 1");
    }

    #[test]
    fn resize_does_not_materialise_dead_slots() {
        // Issue #15: resize cost scales with retained content, not capacity.
        let mut grid = Grid::with_size(80, 24, 100_000);
        grid.resize(120, 30);
        assert_eq!(grid.capacity(), 100_030);
        assert_eq!(grid.total_lines(), 30);
        for r in 0..30 {
            assert_eq!(grid.row(r).unwrap().cells.len(), 120);
        }
        assert!(grid.lines[30..].iter().all(|l| l.cells.is_empty()));
        // Filling the dead slots through the live edge keeps them full width,
        // and scrolling back over them reads complete lines.
        append_lines(&mut grid, 5);
        grid.scroll_up(5);
        for r in 0..30 {
            assert_eq!(grid.row(r).unwrap().cells.len(), 120);
        }
    }

    #[test]
    fn resize_at_100k_scrollback_stays_fast() {
        // Issue #15 measured a 27.8 ms median resize at this capacity before
        // the fix; the loose bound is load headroom, not the target.
        let mut grid = Grid::with_size(80, 24, 100_000);
        let start = std::time::Instant::now();
        for cols in [120, 80] {
            grid.resize(cols, 24);
        }
        let per_resize = start.elapsed() / 2;
        assert!(
            per_resize < std::time::Duration::from_millis(5),
            "resize took {per_resize:?}"
        );
    }

    #[test]
    fn test_resize_widens_retained_lines() {
        // Regression: after widening, writes/selections must index the
        // widened lines, not the old 80-cell layout (was an OOB panic).
        let mut grid = Grid::with_size(80, 4, 10);
        append_lines(&mut grid, 4);
        grid.resize(120, 4);
        for v in 0..4 {
            let line = grid.line_abs(grid.evicted + v).unwrap();
            assert_eq!(line.cells.len(), 120);
        }
        // Write at a column beyond the old width: used to panic.
        let w = Cell {
            char: 'W',
            ..Cell::blank()
        };
        let z = Cell {
            char: 'Z',
            ..Cell::blank()
        };
        grid.write_cell(0, 100, w);
        grid.write_cell(1, 119, z);
        assert_eq!(
            grid.line_text(0).unwrap(),
            "line 1".to_string() + &" ".repeat(94) + "W"
        );
        assert!(grid.line_text(1).unwrap().ends_with('Z'));
        // Selections spanning the widened area stay in bounds. Pos.row is an
        // absolute line id: the viewport shows abs 4..7.
        let sel = Selection::between(Pos { row: 4, col: 0 }, Pos { row: 5, col: 119 });
        let text = grid.selection_text(&sel);
        assert!(text.starts_with("line 1"));
        assert!(text.ends_with('Z'));
        // Word range past the old width is safe.
        let abs = grid.abs_of_view(1).unwrap();
        assert_eq!(grid.word_range_abs(abs, 119), Some((119, 120)));
    }

    #[test]
    fn test_resize_grow_backfills_blank_viewport_rows() {
        // Fresh window with little output, then grow the viewport: every
        // viewport row must have a backing line (was: total stayed at the
        // content count, so abs_of_view(bottom rows) was None and mouse
        // selection on the bottom rows silently died).
        let mut grid = Grid::with_size(80, 10, 100);
        grid.write_cell(
            0,
            0,
            Cell {
                char: 'x',
                ..Cell::blank()
            },
        );
        // total is 10 here (the initial viewport). Shrink, add nothing,
        // then grow: content (10) < new rows (20).
        grid.resize(80, 5);
        grid.resize(80, 20);
        assert_eq!(grid.rows, 20);
        assert!(grid.total_lines() >= 20);
        for r in 0..20 {
            assert!(grid.abs_of_view(r).is_some(), "row {r} has no backing line");
            assert_eq!(grid.row(r).map(|l| l.cells.len()), Some(80));
        }
        // Content stays at the top, blanks fill the bottom.
        assert_eq!(grid.row(0).unwrap().cells[0].char, 'x');
        assert_eq!(grid.row(19).unwrap().cells[0].char, ' ');
    }

    #[test]
    fn test_set_scrollback() {
        let mut grid = Grid::with_size(80, 24, 100);
        append_lines(&mut grid, 200);
        grid.set_scrollback(50);
        assert_eq!(grid.scrollback(), 50);
        assert_eq!(grid.capacity(), 74);
        assert_eq!(grid.total_lines(), 74);
        assert_eq!(row_text(&grid, 23), "line 200");
    }

    #[test]
    fn test_dirty_tracking() {
        let mut grid = Grid::with_size(80, 24, 100);
        grid.take_dirty_rows();
        assert!(grid.dirty_rows().is_empty());

        grid.write_line(3, make_line("a", grid.cols));
        grid.write_line(7, make_line("b", grid.cols));
        assert_eq!(grid.dirty_rows(), vec![3, 7]);

        let taken = grid.take_dirty_rows();
        assert_eq!(taken, vec![3, 7]);
        assert!(grid.dirty_rows().is_empty());

        grid.mark_dirty(2);
        assert_eq!(grid.dirty_rows(), vec![2]);

        grid.mark_all_dirty();
        assert_eq!(grid.dirty_rows(), (0..24).collect::<Vec<_>>());
        grid.take_dirty_rows();
        assert!(grid.dirty_rows().is_empty());
    }

    #[test]
    fn test_for_each_dirty_row_matches_dirty_rows() {
        let mut grid = Grid::with_size(80, 24, 100);
        grid.clear_dirty_rows();
        grid.write_line(3, make_line("a", grid.cols));
        grid.mark_dirty(7);
        let mut rows = Vec::new();
        grid.for_each_dirty_row(|row| rows.push(row));
        assert_eq!(rows, grid.dirty_rows());
        grid.clear_dirty_rows();
        assert!(grid.dirty_rows().is_empty());
    }

    #[test]
    fn test_full_dirty_reports_every_row() {
        let mut grid = Grid::with_size(80, 24, 100);
        grid.clear_dirty_rows();
        grid.scroll_up(1);
        assert!(grid.all_dirty());
        let mut count = 0;
        grid.for_each_dirty_row(|_| count += 1);
        assert_eq!(count, 24);
        grid.clear_dirty_rows();
        assert!(!grid.all_dirty());
        grid.for_each_dirty_row(|_| count += 1);
        assert_eq!(count, 24);
    }

    #[test]
    fn scrolling_the_viewport_marks_every_visible_row_dirty() {
        // Issue #13: an append that shifts the bottom-pinned viewport must
        // repaint every visible row, not just the new bottom line.
        let mut grid = Grid::with_size(10, 3, 4);
        grid.write_line(0, make_line("a", 10));
        grid.write_line(1, make_line("b", 10));
        grid.write_line(2, make_line("c", 10));
        grid.clear_dirty_rows();
        assert!(grid.dirty_rows().is_empty());
        grid.append_line(make_line("d", 10));
        assert!(grid.all_dirty());
        assert_eq!(grid.dirty_rows(), vec![0, 1, 2]);
        assert_eq!(row_text(&grid, 0), "b");
        assert_eq!(row_text(&grid, 2), "d");
    }

    #[test]
    fn test_write_line_and_row_access() {
        let mut grid = Grid::with_size(80, 24, 100);
        grid.write_line(2, make_line("hello", grid.cols));
        assert_eq!(row_text(&grid, 2), "hello");
        assert!(grid.row(24).is_none());
        assert!(grid.row_mut(24).is_none());
        assert!(grid.row_mut(0).is_some());
    }

    #[test]
    fn test_write_cell() {
        let mut grid = Grid::with_size(80, 24, 100);
        let cell = Cell {
            char: 'Z',
            fg: Some(Color::rgb(255, 0, 0)),
            bg: None,
            attributes: CellAttribute::BOLD,
            ..Cell::blank()
        };
        grid.write_cell(4, 10, cell.clone());
        let line = grid.row(4).unwrap();
        assert_eq!(line.cells[10], cell);
    }

    #[test]
    fn test_link_id_survives_scrollback_eviction() {
        let mut grid = Grid::with_size(8, 4, 10);
        let linked = Cell {
            char: 'L',
            link: 7,
            ..Cell::blank()
        };
        grid.write_cell(0, 0, linked);
        // Push the top row into scrollback, then scroll the view up to it.
        grid.scroll_region_up(0, 3);
        grid.scroll_up(1);
        let row = grid.row(0).unwrap();
        assert_eq!(row.cells[0].char, 'L');
        assert_eq!(row.cells[0].link, 7);
    }

    #[test]
    fn test_cell_attribute_flags() {
        let attrs = CellAttribute::BOLD | CellAttribute::UNDERLINE;
        assert!(attrs.contains(CellAttribute::BOLD));
        assert!(attrs.contains(CellAttribute::UNDERLINE));
        assert!(!attrs.contains(CellAttribute::ITALIC));
        assert!(CellAttribute::empty().flags() == 0);
        // `without` clears exactly the requested flags
        let cleared = attrs.without(CellAttribute::BOLD);
        assert!(!cleared.contains(CellAttribute::BOLD));
        assert!(cleared.contains(CellAttribute::UNDERLINE));
    }

    #[test]
    fn test_erase_rows() {
        let mut grid = Grid::with_size(10, 5, 20);
        for r in 0..5 {
            grid.write_line(r, make_line(&format!("row {r}"), 10));
        }
        grid.erase_rows(1, 3);
        assert_eq!(row_text(&grid, 0), "row 0");
        assert_eq!(grid.line_text(1).unwrap(), "");
        assert_eq!(grid.line_text(2).unwrap(), "");
        assert_eq!(grid.line_text(3).unwrap(), "");
        assert_eq!(row_text(&grid, 4), "row 4");
    }

    #[test]
    fn test_scroll_region_up_partial() {
        let mut grid = Grid::with_size(10, 5, 20);
        for r in 0..5 {
            grid.write_line(r, make_line(&format!("row {r}"), 10));
        }
        // Region rows 1..=3: content shifts up, bottom of region blanks,
        // rows outside the region are untouched.
        grid.scroll_region_up(1, 3);
        assert_eq!(row_text(&grid, 0), "row 0");
        assert_eq!(row_text(&grid, 1), "row 2");
        assert_eq!(row_text(&grid, 2), "row 3");
        assert_eq!(grid.line_text(3).unwrap(), "");
        assert_eq!(row_text(&grid, 4), "row 4");
    }

    #[test]
    fn test_scroll_region_up_full_pushes_into_scrollback() {
        let mut grid = Grid::with_size(10, 5, 20);
        for r in 0..5 {
            grid.write_line(r, make_line(&format!("row {r}"), 10));
        }
        let before = grid.total_lines();
        grid.scroll_region_up(0, 4);
        // The full-viewport path appends at the live edge (scrollback grows).
        assert_eq!(grid.total_lines(), before + 1);
        assert_eq!(row_text(&grid, 0), "row 1");
        assert_eq!(row_text(&grid, 3), "row 4");
        assert_eq!(grid.line_text(4).unwrap(), "");
        // Scroll up one line: the viewport shows the oldest five buffer
        // lines, "row 0" through "row 4"; the appended blank sits below
        // the viewport.
        grid.scroll_up(1);
        assert_eq!(row_text(&grid, 0), "row 0");
        assert_eq!(row_text(&grid, 4), "row 4");
    }

    #[test]
    fn test_scroll_region_down() {
        let mut grid = Grid::with_size(10, 5, 20);
        for r in 0..5 {
            grid.write_line(r, make_line(&format!("row {r}"), 10));
        }
        // Region rows 1..=3: content shifts down, top of region blanks,
        // the region's bottom row is discarded.
        grid.scroll_region_down(1, 3);
        assert_eq!(row_text(&grid, 0), "row 0");
        assert_eq!(grid.line_text(1).unwrap(), "");
        assert_eq!(row_text(&grid, 2), "row 1");
        assert_eq!(row_text(&grid, 3), "row 2");
        assert_eq!(row_text(&grid, 4), "row 4");
    }

    #[test]
    fn test_region_scroll_marks_dirty() {
        let mut grid = Grid::with_size(10, 5, 20);
        grid.take_dirty_rows();
        grid.scroll_region_up(0, 2);
        assert_eq!(grid.dirty_rows(), vec![0, 1, 2]);
        grid.take_dirty_rows();
        grid.erase_rows(4, 4);
        assert_eq!(grid.dirty_rows(), vec![4]);
    }

    #[test]
    fn absolute_line_ids_survive_eviction() {
        let mut grid = Grid::with_size(10, 4, 2); // capacity 6; blanks abs 0..3
        append_lines(&mut grid, 4); // "line 1..4" = abs 4..7, evicts abs 0..1
        assert_eq!(grid.abs_of_view(0), Some(4)); // viewport shows abs 4..7
        assert_eq!(grid.last_abs(), 7);
        append_lines(&mut grid, 2); // "line 5..6" = abs 8..9, evicts abs 2..3
        assert_eq!(grid.first_abs(), 4);
        assert!(grid.line_abs(2).is_none());
        assert_eq!(grid.line_text_abs(4).as_deref(), Some("line 1"));
        // The viewport's top row is the oldest line still in the viewport.
        let top_abs = grid.abs_of_view(0).unwrap();
        assert_eq!(grid.view_of_abs(top_abs), Some(0));
        assert_eq!(grid.view_of_abs(0), None);
    }

    #[test]
    fn selection_text_joins_rows_and_trims_trailing_blanks() {
        let mut grid = Grid::with_size(10, 4, 20);
        grid.write_line(0, make_line("abc       ", 10));
        grid.write_line(1, make_line("de", 10));
        grid.write_line(2, make_line("   xyz    ", 10));
        let sel = Selection::between(Pos { row: 0, col: 1 }, Pos { row: 2, col: 4 });
        assert_eq!(grid.selection_text(&sel), "bc\nde\n   xy");
    }

    #[test]
    fn selection_text_drops_evicted_rows_and_trailing_newlines() {
        let mut grid = Grid::with_size(10, 4, 2); // capacity 6; blanks abs 0..3
        append_lines(&mut grid, 8); // abs 4..11; capacity keeps abs 6..11
        let sel = Selection::between(Pos { row: 0, col: 0 }, Pos { row: 9, col: 3 });
        let text = grid.selection_text(&sel);
        // Absolute lines 0..5 are evicted; 6..9 remain. Interior lines are
        // taken whole; the last line is clamped to col 3 ("line").
        assert_eq!(text, "line 3\nline 4\nline 5\nline");
        assert!(!text.ends_with('\n'));
    }

    #[test]
    fn word_range_expands_over_path_like_tokens() {
        let mut grid = Grid::with_size(20, 4, 10);
        grid.write_line(0, make_line("cd src/main.rs  x", 20));
        let abs = grid.abs_of_view(0).unwrap();
        assert_eq!(grid.word_range_abs(abs, 3), Some((3, 14))); // src/main.rs
        assert_eq!(grid.word_range_abs(abs, 0), Some((0, 2)));
        assert_eq!(grid.word_range_abs(abs, 14), Some((14, 15)));
    }
    #[test]
    fn clear_scrollback_keeps_live_absolute_ids() {
        let mut grid = Grid::with_size(10, 3, 10);
        append_lines(&mut grid, 20);
        let first_live = grid.live_abs(0).unwrap();
        let last = grid.last_abs();
        grid.clear_scrollback();
        assert_eq!(grid.first_abs(), first_live);
        assert_eq!(grid.last_abs(), last);
        assert_eq!(grid.live_abs(0), Some(first_live));
    }
}
