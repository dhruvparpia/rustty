//! Streaming text export (issue #53, SPEC EXPORT-001/002).
//!
//! Both exports walk the grid's *absolute* line ids and write one line at
//! a time, so peak memory is one line — never the whole selection, the
//! whole command block, or a history clone. Exports beyond the live
//! window clamp to the kept range (evicted lines no longer exist).

use std::io::Write;

use crate::grid::Grid;
use crate::selection::Selection;

/// Write grid lines `from..=to` (inclusive absolute ids, clamped to the
/// kept range) as plain text, one `writeln!` per line (EXPORT-001/002).
pub fn stream_lines(
    grid: &Grid,
    from: usize,
    to: usize,
    out: &mut (impl Write + ?Sized),
) -> std::io::Result<()> {
    let from = from.max(grid.first_abs());
    let to = to.min(grid.last_abs());
    for abs in from..=to {
        let text = grid.line_text_abs(abs).unwrap_or_default();
        writeln!(out, "{text}")?;
    }
    Ok(())
}

/// Write the text a [`Selection`] covers, matching `Grid::selection_text`
/// semantics (partial columns, trailing spaces trimmed) but streamed line
/// by line (EXPORT-001).
pub fn stream_selection(
    grid: &Grid,
    sel: &Selection,
    out: &mut (impl Write + ?Sized),
) -> std::io::Result<()> {
    let start = sel.start.row.max(grid.first_abs());
    let end = sel.end.row.min(grid.last_abs());
    for abs in start..=end {
        let Some((lo, hi)) = sel.cols_on(abs, grid.cols) else {
            continue;
        };
        let Some(line) = grid.line_abs(abs) else {
            continue;
        };
        let hi = hi.min(line.cells.len());
        let text: String = line.cells[lo.min(hi)..hi].iter().map(|c| c.char).collect();
        writeln!(out, "{}", text.trim_end())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Line;
    use crate::selection::Pos;

    fn make_line(content: &str, cols: usize) -> Line {
        let mut line = Line::blank(cols);
        for (i, ch) in content.chars().take(cols).enumerate() {
            line.cells[i].char = ch;
        }
        line
    }

    fn grid_with(lines: &[&str]) -> Grid {
        let cols = 20;
        let mut grid = Grid::with_size(cols, 8, 64);
        // The grid starts with blank viewport rows; push content lines so
        // absolute ids are known and predictable.
        for s in lines {
            grid.append_line(make_line(s, cols));
        }
        grid
    }

    /// Counts `write_all` calls to prove the writer is fed one small
    /// chunk per line (no single whole-buffer write).
    #[derive(Default)]
    struct CountingWriter {
        calls: usize,
        bytes: usize,
        buf: Vec<u8>,
    }

    impl Write for CountingWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.calls += 1;
            self.bytes += data.len();
            self.buf.extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn stream_writes_one_chunk_per_line() {
        let grid = grid_with(&["alpha", "beta", "gamma"]);
        let to = grid.last_abs();
        let from = to - 2; // the three content lines
        let n = to - from + 1;
        let mut w = CountingWriter::default();
        stream_lines(&grid, from, to, &mut w).unwrap();
        // writeln! = write_all of the text + write_all of the newline.
        assert_eq!(w.calls, n * 2);
        let text = String::from_utf8(w.buf).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("alpha"));
        assert_eq!(lines.next(), Some("beta"));
        assert_eq!(lines.next(), Some("gamma"));
    }

    #[test]
    fn stream_clamps_to_kept_range() {
        let mut grid = Grid::with_size(12, 4, 8);
        for i in 0..20 {
            grid.append_line(make_line(&format!("l{i}"), 12));
        }
        // Ask for a range starting long before the kept window.
        let mut w = CountingWriter::default();
        stream_lines(&grid, 0, grid.last_abs(), &mut w).unwrap();
        let text = String::from_utf8(w.buf).unwrap();
        assert_eq!(text.lines().count(), grid.total_lines());
        assert!(text.starts_with("l8"), "{text}"); // evicted lines are gone
    }

    #[test]
    fn selection_export_matches_grid_text() {
        let grid = grid_with(&["hello world   ", "second line", "third"]);
        let first = grid.last_abs() - 2; // first content line
        let sel = Selection::between(
            Pos { row: first, col: 0 },
            Pos {
                row: first + 1,
                col: 6,
            },
        );
        let mut w = CountingWriter::default();
        stream_selection(&grid, &sel, &mut w).unwrap();
        let streamed = String::from_utf8(w.buf).unwrap();
        let whole = grid.selection_text(&sel);
        let whole = whole.trim_end_matches('\n');
        let streamed = streamed.trim_end();
        // Same characters per line (the stream adds one trailing \n).
        assert_eq!(streamed, whole);
    }
}
