//! Bookmarks over a pane's history (issue #53, SPEC BKMK-001..003).
//!
//! A bookmark anchors on the grid's absolute line id, the same stable id
//! search matches and selection use, so the anchor survives wrap, scroll
//! and resize. Scrollback eviction is handled predictably: an anchor
//! below `Grid::first_abs()` is *evicted* — the list marks it and no jump
//! ever points at a dead line.
//!
//! The list lives on the [`crate::pane::Pane`] (anchors are grid-local);
//! the selection index lives in the window layer.

/// One bookmark: the stable history anchor plus the label captured when
/// it was anchored (line text, head-trimmed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    /// Absolute grid line id this bookmark points at.
    pub abs: usize,
    /// Display text captured at anchor time.
    pub label: String,
}

/// Bookmarks per pane; additions past the cap are rejected so the list
/// stays bounded (same spirit as SEARCH-015).
pub const MAX_BOOKMARKS: usize = 256;

/// Label captured for a bookmark from the anchored line's text:
/// whitespace-trimmed, first 80 chars.
#[must_use]
pub fn label_of(text: &str) -> String {
    text.trim().chars().take(80).collect()
}

/// Add `abs` to the list, or remove it when already present (toggle).
/// Returns `true` when a bookmark was added. The list is kept sorted by
/// anchor so the overlay reads top-to-bottom like the scrollback.
pub fn toggle(list: &mut Vec<Bookmark>, abs: usize, label: String) -> bool {
    if let Some(pos) = list.iter().position(|b| b.abs == abs) {
        list.remove(pos);
        return false;
    }
    if list.len() >= MAX_BOOKMARKS {
        return false;
    }
    let pos = list.partition_point(|b| b.abs < abs);
    list.insert(pos, Bookmark { abs, label });
    true
}

/// Is `abs` bookmarked?
#[must_use]
pub fn has(list: &[Bookmark], abs: usize) -> bool {
    list.iter().any(|b| b.abs == abs)
}

/// Was the anchored line evicted from the scrollback? Eviction moves
/// `first_abs` up, so an anchor below it is dead (BKMK-003).
#[must_use]
pub fn is_evicted(abs: usize, first_abs: usize) -> bool {
    abs < first_abs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bm(abs: usize) -> Bookmark {
        Bookmark {
            abs,
            label: format!("line {abs}"),
        }
    }

    #[test]
    fn toggle_adds_then_removes() {
        let mut list = Vec::new();
        assert!(toggle(&mut list, 10, "a".into()));
        assert!(has(&list, 10));
        assert!(!toggle(&mut list, 10, "a".into()));
        assert!(!has(&list, 10));
    }

    #[test]
    fn list_stays_sorted_by_anchor() {
        let mut list = Vec::new();
        for abs in [40, 10, 25, 15] {
            toggle(&mut list, abs, bm(abs).label);
        }
        let abs: Vec<usize> = list.iter().map(|b| b.abs).collect();
        assert_eq!(abs, vec![10, 15, 25, 40]);
    }

    #[test]
    fn cap_rejects_additions() {
        let mut list = Vec::new();
        for abs in 0..MAX_BOOKMARKS {
            assert!(toggle(&mut list, abs, "x".into()));
        }
        assert_eq!(list.len(), MAX_BOOKMARKS);
        assert!(!toggle(&mut list, 999_999, "x".into()));
        assert_eq!(list.len(), MAX_BOOKMARKS);
    }

    #[test]
    fn eviction_is_detected_at_the_boundary() {
        // A line evicted when first_abs moved past it is dead; a line at
        // first_abs is the oldest kept and still live.
        assert!(is_evicted(9, 10));
        assert!(!is_evicted(10, 10));
        assert!(!is_evicted(11, 10));
    }

    #[test]
    fn label_is_trimmed_and_head_truncated() {
        let label = label_of(&format!("  {}  ", "x".repeat(200)));
        assert_eq!(label.chars().count(), 80);
        assert!(label.chars().all(|c| c == 'x'));
        assert_eq!(label_of(" hi "), "hi");
    }
}
