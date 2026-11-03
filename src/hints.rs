//! Keyboard hint mode (issue #52): every visible link and auto-detected
//! file reference gets a short typed label so the keyboard can open or copy
//! it without a pointer (extends SEC-003; auto-detected targets keep the
//! SEC-022 distinction via `is_diag`).
//!
//! Pure logic: the window collects the visible spans, draws the labels as
//! overlay paint (the grid is never modified), and dispatches the actions.

use crate::diagnostics::FileRef;

/// A visible OSC 8 link run (`col_start..col_end`, view coordinates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSpan {
    /// View row of the run.
    pub row: usize,
    /// First column of the run.
    pub col_start: usize,
    /// One past the last column of the run.
    pub col_end: usize,
    /// The link URI.
    pub url: String,
}

/// A visible auto-detected file reference (SEC-022).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagSpan {
    /// View row of the reference.
    pub row: usize,
    /// First column of the reference.
    pub col_start: usize,
    /// One past the last column of the reference.
    pub col_end: usize,
    /// Copy text: `path:line:column`.
    pub target: String,
    /// The parsed reference (for open and diagnostics display).
    pub file_ref: FileRef,
    /// Editor command when the target is openable; `None` = copy only.
    pub open_command: Option<(String, Vec<String>)>,
}

/// One labelled hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    /// Typed label (`a`, `fd`, ...).
    pub label: String,
    /// Text for "copy target" (the URL, or `path:line:column`).
    pub target: String,
    /// Link URI (`None` for auto-detected references).
    pub url: Option<String>,
    /// Auto-detected file reference: drawn in the distinct SEC-022 style.
    pub is_diag: bool,
    /// An Enter press can open the target.
    pub openable: bool,
    /// Editor command for opening a diagnostic target (links open via the
    /// browser path in the window).
    pub open_command: Option<(String, Vec<String>)>,
    /// View row for the badge.
    pub row: usize,
    /// First column for the badge.
    pub col: usize,
}

/// Labels in reading order, prefix-free so a completed label can never
/// block a longer one: up to 26 targets get single letters `a..z`; above
/// that 16 singles `a..p` plus two-letter labels with first letter `q..z`
/// (`qa..zz`). 276 labels; `build_hints` truncates past that.
pub fn assign_labels(count: usize) -> Vec<String> {
    let mut labels = Vec::with_capacity(count);
    if count <= 26 {
        labels.extend(
            (0..count)
                .map(|i| (b'a' + i as u8) as char)
                .map(String::from),
        );
    } else {
        labels.extend((0..16).map(|i| (b'a' + i as u8) as char).map(String::from));
        for i in 16..count {
            let n = i - 16;
            labels.push(format!(
                "{}{}",
                (b'q' + (n / 26) as u8) as char,
                (b'a' + (n % 26) as u8) as char
            ));
        }
    }
    labels
}

/// Build the labelled hint list from the visible spans. Targets outside
/// `visible_rows` (the viewport, scrollback included by a scrolled view)
/// are dropped; auto-detected references that overlap an OSC 8 link are
/// dropped (the explicit link wins, SEC-022). Order: reading order.
pub fn build_hints(links: &[LinkSpan], diags: &[DiagSpan], visible_rows: usize) -> Vec<Hint> {
    let mut targets: Vec<Hint> = Vec::new();
    for l in links.iter().filter(|l| l.row < visible_rows) {
        targets.push(Hint {
            label: String::new(),
            target: l.url.clone(),
            url: Some(l.url.clone()),
            is_diag: false,
            openable: l.url.starts_with("http://") || l.url.starts_with("https://"),
            open_command: None,
            row: l.row,
            col: l.col_start,
        });
    }
    for d in diags.iter().filter(|d| d.row < visible_rows) {
        // SEC-022: an explicit OSC 8 link wins; a detected reference whose
        // span overlaps one gets no label (no double label, no shadow open).
        if links
            .iter()
            .any(|l| l.row == d.row && l.col_start < d.col_end && d.col_start < l.col_end)
        {
            continue;
        }
        targets.push(Hint {
            label: String::new(),
            target: d.target.clone(),
            url: None,
            is_diag: true,
            openable: d.open_command.is_some(),
            open_command: d.open_command.clone(),
            row: d.row,
            col: d.col_start,
        });
    }
    targets.sort_by_key(|t| (t.row, t.col));
    // shortcut: 276 labels (16 singles + 260 two-letter); a viewport that
    // shows more targets than that is degenerate, extras get no label.
    targets.truncate(276);
    let labels = assign_labels(targets.len());
    for (t, label) in targets.iter_mut().zip(labels) {
        t.label = label;
    }
    targets
}

/// One label-key step of the prefix state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The typed prefix is the start of at least one label.
    Prefix(String),
    /// The typed prefix is a complete label: hint `index` is pending.
    Pending(usize),
    /// The key cannot start any label from this prefix: dismiss.
    Invalid,
}

/// Feed one label key to the current `prefix`.
pub fn advance(hints: &[Hint], prefix: &str, ch: char) -> Step {
    let next = format!("{prefix}{ch}");
    if let Some(idx) = hints.iter().position(|h| h.label == next) {
        return Step::Pending(idx);
    }
    if hints.iter().any(|h| h.label.starts_with(&next)) {
        return Step::Prefix(next);
    }
    Step::Invalid
}

/// The pending-state action keys: `c` copies the target, `u` copies the
/// URL. Anything else (Enter included) is handled by the window.
pub fn action_for(ch: char) -> Option<Action> {
    match ch {
        'c' => Some(Action::CopyTarget),
        'u' => Some(Action::CopyUrl),
        _ => None,
    }
}

/// A pending-hint action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Copy the target text (`path:line:column` for diagnostics).
    CopyTarget,
    /// Copy the URL (links only).
    CopyUrl,
}

/// The open hint-mode overlay: the labelled targets plus the label keys
/// typed so far and the pending complete label, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HintState {
    /// The labelled targets (built once when hint mode opens).
    pub hints: Vec<Hint>,
    /// Label keys typed so far.
    pub prefix: String,
    /// Index into `hints` once a complete label is typed.
    pub pending: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::FileRef;

    fn link(row: usize, col_start: usize, col_end: usize, url: &str) -> LinkSpan {
        LinkSpan {
            row,
            col_start,
            col_end,
            url: url.to_string(),
        }
    }

    fn diag(row: usize, col_start: usize, col_end: usize, openable: bool) -> DiagSpan {
        DiagSpan {
            row,
            col_start,
            col_end,
            target: "C:\\dev\\x.rs:10:5".to_string(),
            file_ref: FileRef {
                path: "C:\\dev\\x.rs".to_string(),
                line: 10,
                col: Some(5),
            },
            open_command: openable.then(|| ("code".to_string(), vec!["--goto".to_string()])),
        }
    }

    #[test]
    fn label_assignment_order() {
        let labels = assign_labels(30);
        assert_eq!(&labels[..5], ["a", "b", "c", "d", "e"]);
        assert_eq!(&labels[14..20], ["o", "p", "qa", "qb", "qc", "qd"]);
        assert_eq!(assign_labels(0), Vec::<String>::new());
        assert_eq!(assign_labels(1), ["a"]);
        assert_eq!(assign_labels(26).last().unwrap(), "z");
    }

    #[test]
    fn hints_are_labelled_in_reading_order() {
        let hints = build_hints(
            &[
                link(5, 40, 50, "https://b.example"),
                link(2, 5, 9, "https://a.example"),
            ],
            &[],
            24,
        );
        assert_eq!(
            hints
                .iter()
                .map(|h| (h.label.as_str(), h.row, h.col))
                .collect::<Vec<_>>(),
            vec![("a", 2, 5), ("b", 5, 40)]
        );
    }

    #[test]
    fn scrollback_targets_are_out_of_scope() {
        // The window only passes view rows, but the filter is explicit: a
        // span at or past `visible_rows` never becomes a hint.
        let hints = build_hints(&[link(24, 0, 5, "https://gone.example")], &[], 24);
        assert!(hints.is_empty());
    }

    #[test]
    fn auto_detected_ref_under_a_link_is_dropped() {
        // SEC-022: the explicit OSC 8 link wins; the auto-detected span
        // that overlaps it gets no label (no two labels, no shadow open).
        let hints = build_hints(
            &[link(3, 10, 40, "https://example/cargo")],
            &[diag(3, 22, 35, true)],
            24,
        );
        assert_eq!(hints.len(), 1);
        assert!(!hints[0].is_diag);
    }

    #[test]
    fn non_overlapping_diag_kept_and_distinct() {
        let hints = build_hints(
            &[link(3, 0, 9, "https://example")],
            &[diag(3, 60, 75, true)],
            24,
        );
        assert_eq!(hints.len(), 2);
        assert!(!hints[0].is_diag);
        assert!(hints[1].is_diag);
        assert!(hints[1].openable);
        assert_eq!(hints[1].target, "C:\\dev\\x.rs:10:5");
    }

    #[test]
    fn copy_only_diag_is_not_openable() {
        let hints = build_hints(&[], &[diag(1, 0, 10, false)], 24);
        assert!(!hints[0].openable);
    }

    #[test]
    fn http_links_open_non_http_do_not() {
        let hints = build_hints(
            &[
                link(0, 0, 4, "https://x.example"),
                link(1, 0, 4, "file:///c/tmp"),
            ],
            &[],
            24,
        );
        assert!(hints[0].openable);
        assert!(!hints[1].openable);
    }

    #[test]
    fn prefix_machine_completes_and_rejects() {
        // 30 targets: labels a..p, then qa..qn (two-letter starts q..z).
        let hints = build_hints(
            &(0..30)
                .map(|i| link(i / 2, (i % 2) * 5, (i % 2) * 5 + 4, "https://x.example"))
                .collect::<Vec<_>>(),
            &[],
            24,
        );
        assert_eq!(hints[16].label, "qa");
        assert_eq!(advance(&hints, "", 'a'), Step::Pending(0));
        assert_eq!(advance(&hints, "", 'q'), Step::Prefix("q".into()));
        assert_eq!(advance(&hints, "q", 'a'), Step::Pending(16));
        assert_eq!(advance(&hints, "q", 'n'), Step::Pending(29));
        assert_eq!(advance(&hints, "q", 'o'), Step::Invalid);
        assert_eq!(advance(&hints, "", 'z'), Step::Invalid);
        assert_eq!(advance(&hints, "", '0'), Step::Invalid);
    }

    #[test]
    fn pending_action_keys() {
        assert_eq!(action_for('c'), Some(Action::CopyTarget));
        assert_eq!(action_for('u'), Some(Action::CopyUrl));
        assert_eq!(action_for('a'), None);
    }
}
