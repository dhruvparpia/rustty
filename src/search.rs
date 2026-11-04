//! Search over a tab's grid and scrollback (SPEC SEARCH-001..015).
//!
//! The search runs per active tab: every pane is scanned from the oldest
//! retained line (`Grid::first_abs`) to the newest (`Grid::last_abs`), so
//! matches are found in the visible screen and in scrollback. Results are
//! capped at [`MAX_MATCHES`] so a pathological query cannot grow without
//! bound (SEARCH-015).
//!
//! Plain search, whole-word search and regex search are all implemented as
//! pure Rust. The UI (`crate::window`) owns the query and navigation; the
//! renderer only reads the result list to paint highlights.

use regex::{Regex, RegexBuilder};

use crate::grid::Grid;
use crate::pane::Pane;
use crate::split::Rect;
use crate::tab::Tab;

/// Maximum matches kept for one query (SEARCH-015: no unbounded growth).
pub const MAX_MATCHES: usize = 5000;

/// What a search considers when matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchOptions {
    /// Match case exactly.
    pub case_sensitive: bool,
    /// Require word boundaries around the match.
    pub whole_word: bool,
    /// Treat the query as a regular expression.
    pub regex: bool,
}

/// One match, in absolute grid coordinates and flatten pane order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMatch {
    /// Flatten index of the pane in the active tab.
    pub pane: usize,
    /// Absolute grid line id (monotonic, scrollback-safe).
    pub abs: usize,
    /// First matched column (inclusive).
    pub lo: usize,
    /// Last matched column (exclusive).
    pub hi: usize,
}

/// Search query state plus the current result list.
///
/// Issue #42: the scan is bounded (`SEARCH_SCAN_ROWS` lines per call) and
/// resumable. `refresh` starts a scan and runs the first chunk inline;
/// `step` continues it, driven by the window layer's timer, so typing,
/// scrolling and shell output stay responsive on 10 MB of scrollback.
/// Every restart (query, options, tab, history rewind) bumps [`SearchState::generation`]
/// and clears the results, so a partial result from a previous query, pane
/// or history state is never kept.
#[derive(Debug, Clone, Default)]
pub struct SearchState {
    /// Query text the user typed.
    pub query: String,
    /// Matching options.
    pub options: SearchOptions,
    /// Matches in pane/line order.
    pub matches: Vec<SearchMatch>,
    /// Index of the current match; wraps on next/previous.
    pub current: usize,
    /// Regex compile error, if any (shown in the bar, never a panic).
    pub error: Option<String>,
    /// Scan generation: bumped on every restart. Results from an older
    /// generation do not exist (they are cleared on restart). (`gen` is a
    /// reserved keyword in edition 2024.)
    pub generation: u64,
    /// `true` while a bounded scan is in flight: the window layer keeps
    /// driving [`SearchState::step`] until it finishes.
    pub scanning: bool,
    /// `true` when the scan stopped at [`MAX_MATCHES`]: the count shown
    /// in the bar carries a "+" because more matches may exist.
    pub truncated: bool,
    /// SEARCH-023 (issue #53): lines of context shown above and below the
    /// current match. From `[search] context-lines`; the result view
    /// reads them straight from the grid, so no re-scan is involved.
    pub context_lines: usize,
    /// Pane index to resume the scan from.
    scan_pane: usize,
    /// Next absolute line to scan within `scan_pane` (0 = pane start).
    scan_abs: usize,
    /// Per scanned pane: `(first_abs, last_abs)` recorded when its scan
    /// started. `note_history` compares live grid bounds against these.
    bounds: Vec<(usize, usize)>,
    /// Per scanned pane: content revision recorded when its scan
    /// started. Existing-line overwrites restart the scan (issue #73).
    content_rev: Vec<u64>,
    /// Compiled regex, reused across chunks.
    regex: Option<Regex>,
    /// Identity of the selected match, preserved across rescans so new
    /// output does not yank the selection.
    current_key: Option<SearchMatch>,
}

/// Lines scanned per bounded chunk (issue #42). A chunk costs a few
/// milliseconds on plain search, so a 10 MB scrollback scan streams in
/// ~150 chunks at the timer cadence without blocking the UI thread.
pub const SEARCH_SCAN_ROWS: usize = 2048;

impl SearchState {
    /// Open a search with an empty query.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a search with the `[search]` defaults applied (SEARCH-007..009).
    #[must_use]
    pub fn with_defaults(case_sensitive: bool, whole_word: bool, regex: bool) -> Self {
        Self {
            options: SearchOptions {
                case_sensitive,
                whole_word,
                regex,
            },
            ..Self::default()
        }
    }

    /// Restart the scan for `tab` and run the first bounded chunk inline;
    /// returns `true` when the result changed.
    ///
    /// Every restart (query, options, tab, history rewind) bumps [`Self::gen`]
    /// and clears the results, so partial results from a previous query, pane
    /// or history state are never kept. The selected match's identity is
    /// remembered so a rescan can restore the user's place. A regex compile
    /// failure records [`SearchState::error`] and leaves no matches.
    pub fn refresh(&mut self, tab: Option<&Tab>) -> bool {
        let before = self.matches.len();
        self.current_key = self.matches.get(self.current).copied();
        self.matches.clear();
        self.error = None;
        self.current = 0;
        self.truncated = false;
        self.scan_pane = 0;
        self.scan_abs = 0;
        self.bounds.clear();
        self.content_rev.clear();
        self.regex = None;
        self.scanning = false;
        self.generation = self.generation.wrapping_add(1);

        if self.query.is_empty() {
            return before != 0;
        }
        let Some(tab) = tab else {
            return before != 0;
        };

        if self.options.regex {
            let mut builder = RegexBuilder::new(&self.query);
            builder.case_insensitive(!self.options.case_sensitive);
            match builder.build() {
                Ok(re) => self.regex = Some(re),
                Err(e) => {
                    self.error = Some(e.to_string());
                    return before != 0;
                }
            }
        }

        self.scanning = true;
        self.scan_chunk(tab);
        before != self.matches.len()
    }

    /// Continue the scan by one bounded chunk (issue #42). Returns `true`
    /// when the result list changed, so the window layer repaints. The
    /// window layer calls this from its search timer until `scanning`
    /// becomes false.
    pub fn step(&mut self, tab: Option<&Tab>) -> bool {
        if !self.scanning {
            return false;
        }
        let Some(tab) = tab else {
            self.scanning = false;
            return false;
        };
        let before = self.matches.len();
        self.scan_chunk(tab);
        before != self.matches.len() || !self.scanning
    }

    /// Shell output arrived (issue #42): extend the scan to cover lines
    /// appended past the bounds recorded when the scan started, and restart
    /// wholesale when history rewound (scrollback eviction, alt-screen swap
    /// or clear) so no result refers to a line that no longer exists.
    /// Returns `true` when the result list changed.
    pub fn note_history(&mut self, tab: Option<&Tab>) -> bool {
        if self.query.is_empty() || self.error.is_some() {
            return false;
        }
        let Some(tab) = tab else { return false };
        let mut restart = false;
        let mut resume: Option<(usize, usize, usize, usize)> = None;
        let mut pane_idx = 0usize;
        tab.for_each_pane_ref(&mut |pane: &Pane, _rect: Rect| {
            if let Some(&(first, last)) = self.bounds.get(pane_idx) {
                let grid = pane.grid();
                if grid.first_abs() > first
                    || grid.last_abs() < last
                    || grid.content_rev() != self.content_rev.get(pane_idx).copied().unwrap_or(0)
                {
                    restart = true;
                } else if grid.last_abs() > last && resume.is_none() {
                    resume = Some((pane_idx, grid.first_abs(), last, grid.last_abs()));
                }
            }
            pane_idx += 1;
        });
        if restart {
            self.refresh(Some(tab));
            return true;
        }
        if let Some((p, first, old_last, new_last)) = resume {
            self.matches.retain(|m| m.pane <= p);
            self.current = self.current.min(self.matches.len().saturating_sub(1));
            self.bounds.truncate(p + 1);
            self.content_rev.truncate(p + 1);
            self.bounds[p] = (first, new_last);
            let Some(pane) = tab.pane_at(p) else {
                return true;
            };
            self.content_rev[p] = pane.grid().content_rev();
            self.scan_pane = p;
            if !self.scanning {
                self.scan_abs = old_last + 1;
                self.scanning = true;
            }
            return true;
        }
        false
    }

    /// Scan at most [`SEARCH_SCAN_ROWS`] lines from the resume position,
    /// recording each pane's bounds so [`Self::note_history`] can tell what
    /// the shell appended. Reaching [`MAX_MATCHES`] stops the scan with
    /// [`Self::truncated`]; finishing every pane restores the selected
    /// match by identity.
    fn scan_chunk(&mut self, tab: &Tab) {
        let mut budget = SEARCH_SCAN_ROWS;
        let mut pane_count = 0usize;
        tab.for_each_pane_ref(&mut |pane: &Pane, _rect: Rect| {
            let pane_idx = pane_count;
            pane_count += 1;
            if !self.scanning || budget == 0 || pane_idx < self.scan_pane {
                return;
            }
            let grid = pane.grid();
            if self.bounds.len() <= pane_idx {
                self.bounds.push((grid.first_abs(), grid.last_abs()));
                self.content_rev.push(grid.content_rev());
                self.scan_abs = grid.first_abs();
            }
            let last = self.bounds[pane_idx].1;
            let mut abs = self.scan_abs.max(grid.first_abs());
            while abs <= last && budget > 0 {
                if self.matches.len() >= MAX_MATCHES {
                    self.truncated = true;
                    self.scanning = false;
                    break;
                }
                if let Some(line) = grid.line_abs(abs) {
                    let (text, bounds) = line_text_with_cell_bounds(line);
                    let found = match self.regex.as_ref() {
                        Some(re) => regex_matches(re, &text, self.options.whole_word),
                        None => plain_matches(
                            &text,
                            &self.query,
                            self.options.case_sensitive,
                            self.options.whole_word,
                        ),
                    };
                    for (lo, hi) in found {
                        if self.matches.len() >= MAX_MATCHES {
                            self.truncated = true;
                            self.scanning = false;
                            break;
                        }
                        if lo >= bounds.len() || hi > bounds.len() {
                            continue;
                        }
                        let (lo_col, _lo_end) = bounds[lo];
                        let (_lo_start, hi_col) = bounds[hi - 1];
                        self.matches.push(SearchMatch {
                            pane: pane_idx,
                            abs,
                            lo: lo_col,
                            hi: hi_col,
                        });
                    }
                }
                abs += 1;
                budget -= 1;
            }
            if self.scanning {
                if abs > last {
                    self.scan_pane = pane_idx + 1;
                    self.scan_abs = 0;
                } else {
                    self.scan_abs = abs;
                }
            }
        });
        if self.scanning && self.scan_pane >= pane_count {
            self.scanning = false;
            if let Some(key) = self.current_key.take()
                && let Some(pos) = self.matches.iter().position(|m| *m == key)
            {
                self.current = pos;
            }
        }
    }

    /// The match currently selected, or `None` when there are no matches.
    #[must_use]
    pub fn current_match(&self) -> Option<SearchMatch> {
        self.matches.get(self.current).copied()
    }

    /// Move to the next match, wrapping.
    pub fn next_match(&mut self) -> Option<SearchMatch> {
        if self.matches.is_empty() {
            return None;
        }
        self.current = (self.current + 1) % self.matches.len();
        Some(self.matches[self.current])
    }

    /// Move to the previous match, wrapping.
    pub fn prev_match(&mut self) -> Option<SearchMatch> {
        if self.matches.is_empty() {
            return None;
        }
        self.current = (self.current + self.matches.len() - 1) % self.matches.len();
        Some(self.matches[self.current])
    }

    /// Toggle case sensitivity and re-run the query.
    pub fn toggle_case(&mut self, tab: Option<&Tab>) {
        self.options.case_sensitive = !self.options.case_sensitive;
        self.refresh(tab);
    }

    /// Toggle whole-word matching and re-run the query.
    pub fn toggle_whole_word(&mut self, tab: Option<&Tab>) {
        self.options.whole_word = !self.options.whole_word;
        self.refresh(tab);
    }

    /// Toggle regex matching and re-run the query.
    pub fn toggle_regex(&mut self, tab: Option<&Tab>) {
        self.options.regex = !self.options.regex;
        self.refresh(tab);
    }

    /// Delete the last query character, then re-run the query.
    pub fn backspace(&mut self, tab: Option<&Tab>) {
        let _ = self.query.pop();
        self.refresh(tab);
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '\\' | ':' | '@')
}

fn whole_word(chars: &[char], start: usize, end: usize) -> bool {
    let left = start == 0 || !is_word_char(chars[start - 1]);
    let right = end >= chars.len() || !is_word_char(chars[end]);
    left && right
}

fn char_eq(a: char, b: char, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a.to_lowercase().next() == b.to_lowercase().next()
    }
}

fn plain_matches(
    line: &str,
    query: &str,
    case_sensitive: bool,
    whole_word_mode: bool,
) -> Vec<(usize, usize)> {
    let hay: Vec<char> = line.chars().collect();
    let needle: Vec<char> = query.chars().collect();
    if needle.is_empty() || needle.len() > hay.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for start in 0..=hay.len() - needle.len() {
        let mut ok = true;
        for j in 0..needle.len() {
            if !char_eq(hay[start + j], needle[j], case_sensitive) {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        let end = start + needle.len();
        if whole_word_mode && !whole_word(&hay, start, end) {
            continue;
        }
        out.push((start, end));
    }
    out
}

fn line_text_with_cell_bounds(line: &crate::grid::Line) -> (String, Vec<(usize, usize)>) {
    let mut text = String::new();
    let mut bounds = Vec::new();
    for (i, cell) in line.cells.iter().enumerate() {
        if cell.continuation {
            continue;
        }
        let cell_text = crate::grid::cell_text(cell);
        for ch in cell_text.chars() {
            text.push(ch);
            bounds.push((i, i + usize::from(cell.width)));
        }
    }
    (text, bounds)
}

fn regex_matches(re: &Regex, line: &str, whole_word_mode: bool) -> Vec<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    for m in re.find_iter(line) {
        let start = line[..m.start()].chars().count();
        let end = line[..m.end()].chars().count();
        if end <= start {
            continue;
        }
        if whole_word_mode && !whole_word(&chars, start, end) {
            continue;
        }
        out.push((start, end));
    }
    out
}

// `Grid` is imported for the doc reference and future search-in-grid helpers;
// keep the import explicit so the module boundary reads clearly.
#[allow(dead_code, reason = "SEARCH-003 references Grid scrollback semantics")]
fn _grid_reference(grid: &Grid) -> usize {
    grid.last_abs()
}

/// SEARCH-023 (issue #53): the context window around a match — lines
/// `abs-n..=abs+n` clamped to the kept range, as `(abs, text, is_match)`.
/// Pure grid reads: opening the view, moving between matches, and
/// history arriving never re-scan the query.
#[must_use]
pub fn context_lines(grid: &Grid, abs: usize, n: usize) -> Vec<(usize, String, bool)> {
    let from = abs.saturating_sub(n).max(grid.first_abs());
    let to = abs.saturating_add(n).min(grid.last_abs());
    (from..=to)
        .filter_map(|a| {
            let text = grid.line_text_abs(a)?;
            Some((a, text, a == abs))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Grid, Line};

    fn tab_from_lines(lines: &[&str], cols: usize) -> Tab {
        let mut tab = Tab::with_size(cols, 4, 1000);
        {
            let mut pane = tab.pane_mut();
            for line in lines {
                pane.grid_mut().append_line(Line::blank(cols));
                pane.grid_mut().write_line_stub(3, line);
            }
        }
        tab
    }

    fn tab_with_scrollback(lines: &[&str], cols: usize) -> Tab {
        let mut tab = Tab::with_size(cols, 4, 1000);
        {
            let mut pane = tab.pane_mut();
            for line in lines {
                pane.grid_mut().append_line(Line::blank(cols));
                pane.grid_mut().write_line_stub(2, line);
            }
        }
        tab
    }

    #[test]
    fn plain_search_finds_visible_and_scrolled_text() {
        let tab = tab_with_scrollback(&["alpha", "beta", "alpha beta"], 20);
        let mut search = SearchState::new();
        search.query = "alpha".to_string();
        assert!(search.refresh(Some(&tab)));
        let matches = search.matches;
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].lo, 0);
        assert_eq!(matches[0].hi, 5);
        assert!(matches.iter().all(|m| m.pane == 0));
    }

    #[test]
    fn case_sensitivity_is_toggleable() {
        let tab = tab_from_lines(&["Alpha"], 10);
        let mut search = SearchState::new();
        search.query = "alpha".to_string();
        search.options.case_sensitive = true;
        assert!(!search.refresh(Some(&tab)), "case-sensitive miss");
        search.options.case_sensitive = false;
        assert!(search.refresh(Some(&tab)));
        assert_eq!(search.matches.len(), 1);
    }

    #[test]
    fn whole_word_rejects_embedded_matches() {
        let tab = tab_from_lines(&["foobar foo"], 20);
        let mut search = SearchState::new();
        search.query = "foo".to_string();
        assert!(search.refresh(Some(&tab)));
        assert_eq!(search.matches.len(), 2);
        search.options.whole_word = true;
        assert!(search.refresh(Some(&tab)));
        assert_eq!(search.matches.len(), 1);
        assert_eq!(search.matches[0].lo, 7);
        assert_eq!(search.matches[0].hi, 10);
    }

    #[test]
    fn regex_search_works_with_unicode() {
        let tab = tab_from_lines(&["café 12", "café 34"], 20);
        let mut search = SearchState::new();
        search.query = "café\\s+\\d+".to_string();
        search.options.regex = true;
        assert!(search.refresh(Some(&tab)));
        assert_eq!(search.matches.len(), 2);
        assert_eq!(search.matches[0].lo, 0);
        assert_eq!(search.matches[0].hi, 7);
    }

    #[test]
    fn invalid_regex_is_reported_without_a_panic() {
        let tab = tab_from_lines(&["abc"], 10);
        let mut search = SearchState::new();
        search.query = "[".to_string();
        search.options.regex = true;
        assert!(!search.refresh(Some(&tab)));
        assert!(search.error.is_some());
        assert!(search.matches.is_empty());
    }

    #[test]
    fn navigation_wraps() {
        let tab = tab_from_lines(&["a", "a"], 10);
        let mut search = SearchState::new();
        search.query = "a".to_string();
        search.refresh(Some(&tab));
        assert_eq!(search.current, 0);
        assert_eq!(search.next_match().map(|m| m.abs), Some(5));
        assert_eq!(search.prev_match().map(|m| m.abs), Some(4));
    }

    #[test]
    fn results_are_capped() {
        let tab = tab_from_lines(&["aaaaaaaaaa", "aaaaaaaaaa"], 10);
        let mut search = SearchState::new();
        search.query = "a".to_string();
        search.refresh(Some(&tab));
        assert!(search.matches.len() <= MAX_MATCHES);
    }

    #[test]
    fn empty_query_clears_results() {
        let tab = tab_from_lines(&["abc"], 10);
        let mut search = SearchState::new();
        search.query = "abc".to_string();
        search.refresh(Some(&tab));
        assert!(search.matches.len() == 1);
        search.query.clear();
        search.refresh(Some(&tab));
        assert!(search.matches.is_empty());
    }

    #[test]
    fn grid_reference_is_just_the_last_line() {
        let mut grid = Grid::with_size(10, 2, 10);
        grid.write_line_stub(0, "x");
        assert_eq!(_grid_reference(&grid), grid.last_abs());
    }

    /// A single-pane tab of `n` filler lines; the last line is `needle`.
    fn tab_with_needle_last(n: usize, cols: usize) -> Tab {
        let mut tab = Tab::with_size(cols, 4, n + 100);
        {
            let mut pane = tab.pane_mut();
            for i in 0..n {
                pane.grid_mut().append_line(Line::blank(cols));
                let text = if i + 1 == n { "needle" } else { "filler" };
                pane.grid_mut().write_line_stub(3, text);
            }
        }
        tab
    }

    fn append_line(tab: &mut Tab, text: &str, _row: usize) {
        let cols = tab.cols();
        let mut line = Line::blank(cols);
        for (i, ch) in text.chars().take(cols).enumerate() {
            line.cells[i].char = ch;
        }
        let mut pane = tab.pane_mut();
        pane.grid_mut().append_line(line);
    }

    /// Drain the bounded scan to completion; returns how many steps it took.
    fn drain_scan(search: &mut SearchState, tab: &Tab) -> usize {
        let mut steps = 0;
        while search.scanning {
            search.step(Some(tab));
            steps += 1;
            assert!(steps < 1000, "scan must terminate");
        }
        steps
    }

    #[test]
    fn big_scan_is_bounded_and_continues_to_completion() {
        // Issue #42: refresh runs one bounded chunk; the window layer's
        // timer drains the rest, and the last line's match is found.
        let n = SEARCH_SCAN_ROWS * 3 + 7;
        let tab = tab_with_needle_last(n, 10);
        let mut search = SearchState::new();
        search.query = "needle".to_string();
        search.refresh(Some(&tab));
        assert!(search.scanning, "scan is still in flight after refresh");
        assert!(
            search.matches.is_empty(),
            "first chunk cannot reach the end"
        );
        let steps = drain_scan(&mut search, &tab);
        assert!(steps >= 2, "resumed in chunks, not one full walk");
        assert_eq!(search.matches.len(), 1);
        assert!(!search.truncated);
    }

    #[test]
    fn cap_stops_the_scan_and_marks_truncated() {
        let n = SEARCH_SCAN_ROWS * 3 + 7;
        let tab = tab_with_needle_last(n, 10);
        let mut search = SearchState::new();
        search.query = "filler".to_string();
        search.refresh(Some(&tab));
        drain_scan(&mut search, &tab);
        assert_eq!(search.matches.len(), MAX_MATCHES);
        assert!(search.truncated, "count carries a \"+\"");
        assert!(!search.scanning, "the cap stops the scan");
    }

    #[test]
    fn new_output_extends_a_finished_scan() {
        // Issue #42: appended lines are searched without a full rescan,
        // and the matches already shown stay in place.
        let mut tab = tab_from_lines(&["alpha"], 10);
        let mut search = SearchState::new();
        search.query = "alpha".to_string();
        search.refresh(Some(&tab));
        assert!(!search.scanning);
        append_line(&mut tab, "alpha again", 3);
        assert!(search.note_history(Some(&tab)));
        assert!(search.scanning, "the scan resumes for the new tail");
        assert_eq!(search.matches.len(), 1, "the known match survives");
        drain_scan(&mut search, &tab);
        assert_eq!(search.matches.len(), 2);
    }

    #[test]
    fn eviction_restarts_the_scan() {
        // Issue #42: once recorded lines are evicted, no result may refer
        // to a dead line id: the scan restarts over the retained history.
        let mut tab = Tab::with_size(10, 4, 4);
        append_line(&mut tab, "needle", 2);
        for _ in 0..4 {
            append_line(&mut tab, "filler", 3);
        }
        let mut search = SearchState::new();
        search.query = "needle".to_string();
        search.refresh(Some(&tab));
        assert_eq!(search.matches.len(), 1);
        for _ in 0..4 {
            append_line(&mut tab, "filler", 3);
        }
        assert!(search.note_history(Some(&tab)), "eviction restarts");
        drain_scan(&mut search, &tab);
        assert!(search.matches.is_empty(), "the evicted match is gone");
    }

    #[test]
    fn query_change_drops_stale_partial_results() {
        // Issue #42: results from an older query/generation never survive;
        // each restart bumps the generation and clears the list.
        let n = SEARCH_SCAN_ROWS * 3 + 7;
        let tab = tab_with_needle_last(n, 10);
        let mut search = SearchState::new();
        search.query = "filler".to_string();
        search.refresh(Some(&tab));
        search.step(Some(&tab));
        assert!(!search.matches.is_empty(), "partial results exist");
        let generation = search.generation;
        search.query = "needle".to_string();
        search.refresh(Some(&tab));
        assert_eq!(search.generation, generation + 1);
        assert!(search.matches.is_empty(), "stale results are dropped");
        assert!(search.scanning);
        drain_scan(&mut search, &tab);
        assert_eq!(search.matches.len(), 1, "only the new query's match");
    }

    #[test]
    fn rescan_restores_the_selected_match() {
        // Issue #42: a toggle re-runs the query; the selection keeps its
        // place when the same match is found again.
        let tab = tab_from_lines(&["a x", "b y", "c x"], 10);
        let mut search = SearchState::new();
        search.query = "x".to_string();
        search.refresh(Some(&tab));
        assert_eq!(search.matches.len(), 2);
        search.current = 1;
        let key = search.matches[1];
        search.query = "X".to_string(); // case-insensitive: same matches
        search.refresh(Some(&tab));
        drain_scan(&mut search, &tab);
        assert_eq!(search.matches.len(), 2);
        assert_eq!(search.current_match(), Some(key));
    }

    #[test]
    fn context_window_around_a_match() {
        // Issue #53 (SEARCH-023): the view is pure grid reads around the
        // match's abs; the match row is flagged, neighbours are not.
        let mut grid = Grid::with_size(10, 2, 10);
        for i in 0..6 {
            let mut line = Line::blank(10);
            for (c, ch) in format!("l{i}").chars().enumerate() {
                line.cells[c].char = ch;
            }
            grid.append_line(line);
        }
        let abs = grid.last_abs() - 2;
        let ctx = context_lines(&grid, abs, 2);
        let ids: Vec<usize> = ctx.iter().map(|(a, ..)| *a).collect();
        assert_eq!(ids, (abs - 2..=abs + 2).collect::<Vec<_>>());
        let flags: Vec<bool> = ctx.iter().map(|(_, _, m)| *m).collect();
        assert_eq!(flags, vec![false, false, true, false, false]);
        assert_eq!(ctx[2].1, format!("l{}", abs - 2)); // viewport rows precede content
    }

    #[test]
    fn context_window_clamps_at_both_ends() {
        let mut grid = Grid::with_size(10, 2, 10);
        for _ in 0..4 {
            grid.append_line(Line::blank(10));
        }
        // Oldest kept line with n=3: only the line itself and the 3 below.
        let first = grid.first_abs();
        let ctx = context_lines(&grid, first, 3);
        assert_eq!(ctx.len(), 4);
        assert!(ctx[0].2);
        // The newest line with n=3: only the 3 above and itself.
        let last = grid.last_abs();
        let ctx = context_lines(&grid, last, 3);
        assert_eq!(ctx.len(), 4);
        assert!(ctx[3].2);
    }
    #[test]
    fn search_columns_are_grid_cells_after_wide_text() {
        let mut tab = Tab::with_size(8, 2, 0);
        tab.pane_mut().feed("界X".as_bytes());
        let mut search = SearchState::new();
        search.query = "X".to_string();
        search.refresh(Some(&tab));
        let Some(found) = search.current_match() else {
            panic!("wide-text match must be found");
        };
        assert_eq!((found.lo, found.hi), (2, 3));
    }

    #[test]
    fn continuous_output_reaches_live_tail_without_restart() {
        let mut tab = Tab::with_size(30, 4, 10000);
        for _ in 0..6500 {
            tab.pane_mut().feed(b"old line\r\n");
        }
        tab.pane_mut().feed(b"NEEDLE\r\n");
        let mut search = SearchState::new();
        search.query = "NEEDLE".to_string();
        search.refresh(Some(&tab));
        for _ in 0..20 {
            tab.pane_mut().feed(b"new line\r\n");
            search.note_history(Some(&tab));
            search.step(Some(&tab));
        }
        assert!(
            !search.matches.is_empty(),
            "tail starved after 20 scan ticks"
        );
    }
}
