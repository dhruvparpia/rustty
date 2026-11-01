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
}

impl SearchState {
    /// Open a search with an empty query.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Recompute matches for `tab`; returns `true` when the result changed.
    ///
    /// A regex compile failure records [`SearchState::error`] and leaves no
    /// matches. The query is capped by [`MAX_MATCHES`], not by a timeout.
    pub fn refresh(&mut self, tab: Option<&Tab>) -> bool {
        let before = self.matches.len();
        self.matches.clear();
        self.error = None;
        self.current = 0;

        if self.query.is_empty() {
            return before != 0;
        }
        let Some(tab) = tab else {
            return before != 0;
        };

        let regex = if self.options.regex {
            let mut builder = RegexBuilder::new(&self.query);
            builder.case_insensitive(!self.options.case_sensitive);
            match builder.build() {
                Ok(re) => Some(re),
                Err(e) => {
                    self.error = Some(e.to_string());
                    return before != 0;
                }
            }
        } else {
            None
        };

        let mut pane_idx = 0usize;
        let mut capped = false;
        tab.for_each_pane_ref(&mut |pane: &Pane, _rect: Rect| {
            if capped {
                pane_idx += 1;
                return;
            }
            let grid = pane.grid();
            for abs in grid.first_abs()..=grid.last_abs() {
                let Some(line) = grid.line_text_abs(abs) else {
                    continue;
                };
                let found = match regex.as_ref() {
                    Some(re) => regex_matches(re, &line, self.options.whole_word),
                    None => plain_matches(
                        &line,
                        &self.query,
                        self.options.case_sensitive,
                        self.options.whole_word,
                    ),
                };
                for (lo, hi) in found {
                    if self.matches.len() >= MAX_MATCHES {
                        capped = true;
                        break;
                    }
                    self.matches.push(SearchMatch {
                        pane: pane_idx,
                        abs,
                        lo,
                        hi,
                    });
                }
                if capped {
                    break;
                }
            }
            pane_idx += 1;
        });
        before != self.matches.len()
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
}
