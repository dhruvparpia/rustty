//! Command palette (issue #39): a fuzzy, keyboard-first launcher for every
//! command, built from one action registry shared with menus and keybindings.
//!
//! The window layer owns the item list (it knows tabs, profiles, and which
//! commands have nothing to act on) and the renderer paints it. This module
//! is pure state: the query, the scored hits, and the built-in shortcut
//! table that drives both the palette's shortcut labels and the
//! keybinding-conflict diagnostics.

use std::collections::BTreeMap;

use crate::config::KeyAction;

/// Maximum hits kept for one query. Typing cannot grow the hit list beyond
/// this bound, so a pathological query cannot stall a frame.
pub const MAX_HITS: usize = 200;

/// Rows of hits visible in the overlay (plus the header row).
pub const VISIBLE_ROWS: usize = 9;

/// Maximum recent commands kept (issue #39 acceptance: "a bounded number of
/// recent commands"). Newest first.
pub const MAX_RECENT: usize = 10;

/// What a palette row runs. `Action` reuses the exact [`KeyAction`] the
/// menus and keybindings dispatch, so one registry drives all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// A standard action, identical to a keybinding target.
    Action(KeyAction),
    /// Open a new tab running profile `n` (index into the merged profile list).
    NewTabProfile(usize),
    /// Switch to tab `n`.
    SwitchTab(usize),
    /// Open a tab that views live host session `n` (issue #49).
    AttachSession(u32),
    /// Close the active session-view tab; this detaches the view, not the shell.
    DetachView,
    /// Ask the active session view to take controller rights.
    TakeControlView,
    /// Terminate the active session view's host shell.
    TerminateView,
    /// Run project task `n` (issue #89).
    RunTask(u32),
    /// Stop project task `n`.
    StopTask(u32),
    /// Stop and restart project task `n`.
    RestartTask(u32),
    /// Focus pane `n` in the active tab.
    FocusPane(usize),
    /// Open saved workspace `n` (replaces this window's tabs).
    OpenWorkspace(usize),
    /// Switch to theme `n`.
    SwitchTheme(usize),
    /// Open existing Git worktree `n` as a tab at its root.
    OpenWorktree(usize),
    /// Run a window menu command by id, for commands with no [`KeyAction`].
    MenuItem(u32),
    /// Open the native Preferences config editor.
    OpenPreferences,
    /// Open the in-app settings overlay at setting `n` (issue #91).
    OpenSettings(usize),
    /// Jump the viewport to command `n`'s block.
    JumpCommand(u64),
    /// Copy one command's output by canonical id.
    CopyCommand(u64),
    /// Export one command's output by canonical id.
    ExportCommand(u64),
    /// Search one finished command's output by canonical id (issue #90).
    SearchCommand(u64),
}

/// One command row.
#[derive(Debug, Clone)]
pub struct PaletteItem {
    /// User-visible label, e.g. "New Tab".
    pub label: String,
    /// Group heading shown in the list ("Recent", "Tabs", "Profiles", ...).
    pub group: &'static str,
    /// Shortcut to display, already normalized, `None` when the command has
    /// no key binding.
    pub shortcut: Option<String>,
    /// Why this command is unavailable right now, `None` when runnable.
    /// The palette shows disabled rows with this reason (issue #39
    /// acceptance: "disabled actions explain why").
    pub disabled_reason: Option<String>,
    /// What runs on Enter.
    pub command: Command,
}

/// The open palette: query, item registry, scored hits, selected row.
#[derive(Debug, Clone)]
pub struct PaletteState {
    /// Typed query.
    pub query: String,
    /// Every command, in registration order.
    items: Vec<PaletteItem>,
    /// Indices into `items`, best score first (capped at [`MAX_HITS`]).
    hits: Vec<usize>,
    /// Selected position within `hits`.
    pub current: usize,
}

impl PaletteState {
    /// Build the palette from a fresh item registry. `recent` lists labels
    /// of recently run commands, newest first; matching items are re-grouped
    /// under "Recent" at the top of the registry.
    #[must_use]
    pub fn new(items: Vec<PaletteItem>, recent: &[String]) -> Self {
        let mut items = items;
        // Re-group recents: move matching items to the front of the list,
        // newest first, preserving each item's identity (index-based hits).
        let mut reordered: Vec<PaletteItem> = Vec::with_capacity(items.len());
        for label in recent.iter().take(MAX_RECENT) {
            if let Some(pos) = items.iter().position(|it| &it.label == label) {
                let mut it = items.remove(pos);
                it.group = "Recent";
                reordered.push(it);
            }
        }
        reordered.append(&mut items);
        let mut state = Self {
            query: String::new(),
            items: reordered,
            hits: Vec::new(),
            current: 0,
        };
        state.refresh();
        state
    }

    /// Re-append the query with one printable character.
    pub fn push_char(&mut self, ch: char) {
        if !ch.is_control() {
            self.query.push(ch);
        }
    }

    /// Remove the last query character.
    pub fn backspace(&mut self) {
        self.query.pop();
    }

    /// Recompute the hit list for the current query and clamp the selection.
    pub fn refresh(&mut self) {
        let mut scored: Vec<(i64, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(idx, it)| fuzzy_score(&it.label, &self.query).map(|s| (s, idx)))
            .collect();
        // Best score first; registration order breaks ties so the list is
        // stable while typing.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.truncate(MAX_HITS);
        self.hits = scored.into_iter().map(|(_, idx)| idx).collect();
        self.current = self.current.min(self.hits.len().saturating_sub(1));
    }

    /// The hit list (indices into `items`).
    #[must_use]
    pub fn hits(&self) -> &[usize] {
        &self.hits
    }

    /// The item at registry index `idx`.
    #[must_use]
    pub fn item(&self, idx: usize) -> Option<&PaletteItem> {
        self.items.get(idx)
    }

    /// The full registry (for refresh/selection identity).
    #[must_use]
    pub fn registry(&self) -> &[PaletteItem] {
        &self.items
    }

    /// The selected item, `None` when the hit list is empty.
    #[must_use]
    pub fn selected(&self) -> Option<&PaletteItem> {
        self.hits
            .get(self.current)
            .copied()
            .and_then(|i| self.item(i))
    }

    /// Move the selection by `delta` within the hit list.
    pub fn step(&mut self, delta: i32) {
        if self.hits.is_empty() {
            return;
        }
        let n = i64::try_from(self.hits.len()).unwrap_or(i64::MAX);
        let cur = i64::try_from(self.current).unwrap_or(0);
        let next = (cur + i64::from(delta)).clamp(0, n - 1);
        self.current = usize::try_from(next).unwrap_or(0);
    }

    /// The first hit index to render so that `current` stays visible within
    /// `rows` slots.
    #[must_use]
    pub fn window_start(&self, rows: usize) -> usize {
        self.current.saturating_sub(rows.saturating_sub(1))
    }

    /// Record `label` as the most recent command, de-duplicating and
    /// capping the list (issue #39: bounded recents).
    pub fn record_recent(recent: &mut Vec<String>, label: &str) {
        recent.retain(|l| l != label);
        recent.insert(0, label.to_string());
        recent.truncate(MAX_RECENT);
    }
}

/// Case-insensitive fuzzy subsequence score: every query character must
/// appear in `label` in order. Higher is better: prefix matches and matches
/// at word starts beat mid-word hits, and early matches beat late ones.
/// `None` when the query is not a subsequence.
#[must_use]
pub fn fuzzy_score(label: &str, query: &str) -> Option<i64> {
    let query = query.trim();
    if query.is_empty() {
        return Some(0);
    }
    let label_lower: Vec<char> = label.to_lowercase().chars().collect();
    let query_chars: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let mut score: i64 = 0;
    let mut pos = 0usize;
    for (qi, qch) in query_chars.iter().enumerate() {
        let mut found = usize::MAX;
        for (i, c) in label_lower.iter().enumerate().skip(pos) {
            if *c == *qch {
                found = i;
                break;
            }
        }
        if found == usize::MAX {
            return None;
        }
        let at = found;
        pos = at + 1;
        if qi == 0 && at == 0 {
            score += 24;
        }
        let prev = if at == 0 { ' ' } else { label_lower[at - 1] };
        if prev.is_whitespace() || prev == ':' || prev == '-' || prev == '/' {
            score += 12;
        }
        score += 4 - i64::try_from(at.min(4)).unwrap_or(4);
    }
    Some(score)
}

/// The built-in default shortcut for each action hardcoded in the window
/// layer (the table the palette displays and the conflict diagnostics
/// compare user bindings against). `None` for actions with no default key.
#[must_use]
pub fn builtin_shortcut(action: KeyAction) -> Option<&'static str> {
    Some(match action {
        KeyAction::NewTab => "ctrl+shift+t",
        KeyAction::CloseTab => "ctrl+shift+w",
        KeyAction::Copy => "ctrl+shift+c",
        KeyAction::Paste => "ctrl+shift+v",
        KeyAction::Search => "ctrl+f",
        KeyAction::Fullscreen => "f11",
        KeyAction::NextTab => "ctrl+shift+right-bracket",
        KeyAction::PrevTab => "ctrl+shift+left-bracket",
        KeyAction::SplitRight => "ctrl+shift+enter",
        KeyAction::SplitDown => "ctrl+shift+minus",
        KeyAction::FocusUp => "ctrl+shift+up",
        KeyAction::FocusDown => "ctrl+shift+down",
        KeyAction::FocusLeft => "ctrl+shift+left",
        KeyAction::FocusRight => "ctrl+shift+right",
        KeyAction::CommandPalette => "ctrl+shift+p",
        KeyAction::HintMode => "ctrl+shift+k",
        KeyAction::ToggleBookmark => "ctrl+shift+b",
        KeyAction::ToggleSessionRail => "ctrl+shift+s",
        KeyAction::ToggleBoard => "ctrl+shift+r",
        _ => return None,
    })
}

/// Human-readable display form of a normalized key spec ("ctrl+shift+p" →
/// "Ctrl+Shift+P").
#[must_use]
pub fn display_shortcut(spec: &str) -> String {
    spec.split('+')
        .map(|part| match part {
            "ctrl" => "Ctrl".to_string(),
            "shift" => "Shift".to_string(),
            "alt" => "Alt".to_string(),
            "enter" => "Enter".to_string(),
            "escape" => "Esc".to_string(),
            "left-bracket" => "[".to_string(),
            "right-bracket" => "]".to_string(),
            "minus" => "-".to_string(),
            "space" => "Space".to_string(),
            other => {
                let mut chars = other.chars();
                match chars.next() {
                    Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Diagnostics for `[keybindings]` that collide with a built-in shortcut
/// (issue #39: "keybinding conflict diagnostics"). Each message names the
/// binding, the built-in it shadows, and which one wins (the user binding,
/// by KEY-022..025 precedence).
#[must_use]
pub fn keybinding_conflicts(map: &BTreeMap<String, KeyAction>) -> Vec<String> {
    let mut out = Vec::new();
    for (spec, action) in map {
        if let Some(builtin) = builtin_shortcut(*action)
            && spec == builtin
        {
            // The binding re-states the built-in default: no conflict.
            continue;
        }
        for (builtin_spec, builtin_action) in builtin_actions() {
            if spec == builtin_spec && *action != builtin_action {
                out.push(format!(
                    "keybinding: \"{spec}\" shadows the built-in shortcut for {} \
                     (built-in: {}); the user binding wins",
                    action_name(*action),
                    action_name(builtin_action)
                ));
            }
        }
    }
    out
}

/// The built-in shortcuts as (spec, action) pairs.
fn builtin_actions() -> impl Iterator<Item = (&'static str, KeyAction)> {
    use KeyAction::*;
    [
        NewTab,
        CloseTab,
        Copy,
        Paste,
        Search,
        Fullscreen,
        NextTab,
        PrevTab,
        SplitRight,
        SplitDown,
        SplitLeft,
        SplitUp,
        MovePaneLeft,
        MovePaneRight,
        MovePaneUp,
        MovePaneDown,
        FocusUp,
        FocusDown,
        FocusLeft,
        FocusRight,
        CommandPalette,
        HintMode,
        ToggleBookmark,
        ToggleSessionRail,
        ToggleBoard,
    ]
    .into_iter()
    .filter_map(|a| builtin_shortcut(a).map(|s| (s, a)))
}

/// Short human label for an action, used in conflict messages.
fn action_name(action: KeyAction) -> &'static str {
    match action {
        KeyAction::NewTab => "new-tab",
        KeyAction::CloseTab => "close-tab",
        KeyAction::NextTab => "next-tab",
        KeyAction::PrevTab => "prev-tab",
        KeyAction::Copy => "copy",
        KeyAction::Paste => "paste",
        KeyAction::Search => "search",
        KeyAction::SplitRight => "split-right",
        KeyAction::SplitDown => "split-down",
        KeyAction::SplitLeft => "split-left",
        KeyAction::SplitUp => "split-up",
        KeyAction::MovePaneLeft => "move-pane-left",
        KeyAction::MovePaneRight => "move-pane-right",
        KeyAction::MovePaneUp => "move-pane-up",
        KeyAction::MovePaneDown => "move-pane-down",
        KeyAction::Fullscreen => "fullscreen",
        KeyAction::Quit => "quit",
        KeyAction::FocusUp => "focus-up",
        KeyAction::FocusDown => "focus-down",
        KeyAction::FocusLeft => "focus-left",
        KeyAction::FocusRight => "focus-right",
        KeyAction::ResizeUp => "resize-up",
        KeyAction::ResizeDown => "resize-down",
        KeyAction::ResizeLeft => "resize-left",
        KeyAction::ResizeRight => "resize-right",
        KeyAction::NextProfile => "next-profile",
        KeyAction::PrevProfile => "prev-profile",
        KeyAction::NextTheme => "next-theme",
        KeyAction::PrevTheme => "prev-theme",
        KeyAction::MoveTabLeft => "move-tab-left",
        KeyAction::MoveTabRight => "move-tab-right",
        KeyAction::RenameTab => "rename-tab",
        KeyAction::CloseQuick => "close-quick",
        KeyAction::IncreaseFontSize => "increase-font-size",
        KeyAction::DecreaseFontSize => "decrease-font-size",
        KeyAction::ResetFontSize => "reset-font-size",
        KeyAction::ReloadConfig => "reload-config",
        KeyAction::ClearScreen => "clear-screen",
        KeyAction::PrevCommand => "prev-command",
        KeyAction::NextCommand => "next-command",
        KeyAction::CopyLastCommand => "copy-last-command",
        KeyAction::CommandPalette => "command-palette",
        KeyAction::HintMode => "hint-mode",
        KeyAction::ToggleBookmark => "toggle-bookmark",
        KeyAction::ToggleSessionRail => "toggle-session-rail",
        KeyAction::ToggleBoard => "toggle-board",
        KeyAction::NewWorktreeSession => "new-worktree-session",
        KeyAction::InitProjectBrain => "init-project-brain",
        KeyAction::StartBoss => "start-boss",
        KeyAction::ShowBookmarks => "show-bookmarks",
        KeyAction::ExportSelection => "export-selection",
        KeyAction::ExportLastCommand => "export-last-command",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, command: Command) -> PaletteItem {
        PaletteItem {
            label: label.to_string(),
            group: "Actions",
            shortcut: None,
            disabled_reason: None,
            command,
        }
    }

    #[test]
    fn fuzzy_matches_subsequences_in_order() {
        assert!(fuzzy_score("New Tab", "nt").is_some());
        assert!(fuzzy_score("New Tab", "tb").is_some());
        assert!(fuzzy_score("New Tab", "bt").is_none(), "out of order");
        assert!(fuzzy_score("New Tab", "new tab x").is_none());
    }

    #[test]
    fn fuzzy_is_case_insensitive() {
        assert!(fuzzy_score("New Tab", "NEW").is_some());
        assert!(fuzzy_score("new tab", "NT").is_some());
    }

    #[test]
    fn fuzzy_prefers_prefix_and_word_starts() {
        let prefix = fuzzy_score("Paste", "pa").unwrap();
        let mid = fuzzy_score("Copy Palette", "pa").unwrap();
        assert!(prefix > mid, "prefix {prefix} should beat mid-word {mid}");
    }

    #[test]
    fn refresh_sorts_best_first_and_clamps() {
        let items = vec![
            item("Switch to Tab: paste", Command::SwitchTab(0)),
            item("Paste", Command::Action(KeyAction::Paste)),
        ];
        let mut state = PaletteState::new(items, &[]);
        state.query = "pa".into();
        state.refresh();
        assert_eq!(state.hits().len(), 2);
        assert_eq!(state.item(state.hits()[0]).unwrap().label, "Paste");
    }

    #[test]
    fn empty_query_lists_everything() {
        let items = vec![
            item("Copy", Command::Action(KeyAction::Copy)),
            item("Paste", Command::Action(KeyAction::Paste)),
        ];
        let state = PaletteState::new(items, &[]);
        assert_eq!(state.hits().len(), 2);
    }

    #[test]
    fn hits_are_capped() {
        let items: Vec<PaletteItem> = (0..MAX_HITS + 50)
            .map(|i| item(&format!("Command {i}"), Command::SwitchTab(i)))
            .collect();
        let mut state = PaletteState::new(items, &[]);
        state.query = "command".into();
        state.refresh();
        assert!(state.hits().len() <= MAX_HITS);
    }

    #[test]
    fn step_clamps_to_the_hit_list() {
        let items = vec![
            item("A", Command::SwitchTab(0)),
            item("B", Command::SwitchTab(1)),
        ];
        let mut state = PaletteState::new(items, &[]);
        state.step(-5);
        assert_eq!(state.current, 0);
        state.step(9);
        assert_eq!(state.current, 1);
    }

    #[test]
    fn recent_regroups_matching_items_first_capped() {
        let items = vec![
            item("Copy", Command::Action(KeyAction::Copy)),
            item("Paste", Command::Action(KeyAction::Paste)),
            item("Quit", Command::Action(KeyAction::Quit)),
        ];
        let recent = vec!["Paste".to_string(), "Copy".to_string()];
        let state = PaletteState::new(items, &recent);
        assert_eq!(state.item(0).unwrap().label, "Paste");
        assert_eq!(state.item(0).unwrap().group, "Recent");
        assert_eq!(state.item(1).unwrap().label, "Copy");
    }

    #[test]
    fn record_recent_dedupes_and_caps() {
        let mut recent: Vec<String> = Vec::new();
        for _ in 0..MAX_RECENT + 5 {
            PaletteState::record_recent(&mut recent, "Copy");
        }
        assert_eq!(recent.len(), 1);
        for i in 0..MAX_RECENT + 5 {
            PaletteState::record_recent(&mut recent, &format!("Cmd {i}"));
        }
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(recent[0], format!("Cmd {}", MAX_RECENT + 4));
    }

    #[test]
    fn conflicts_flag_shadowed_builtins() {
        let mut map = BTreeMap::new();
        map.insert("ctrl+shift+t".to_string(), KeyAction::Quit);
        map.insert("ctrl+shift+p".to_string(), KeyAction::CommandPalette);
        let conflicts = keybinding_conflicts(&map);
        assert_eq!(conflicts.len(), 1, "restating a default is not a conflict");
        assert!(conflicts[0].contains("ctrl+shift+t"));
    }

    #[test]
    fn display_shortcut_is_human_readable() {
        assert_eq!(display_shortcut("ctrl+shift+p"), "Ctrl+Shift+P");
        assert_eq!(display_shortcut("ctrl+shift+right-bracket"), "Ctrl+Shift+]");
    }
    #[test]
    fn fuzzy_handles_unicode_boundaries() {
        assert!(fuzzy_score("résumé", "rés").is_some());
        assert!(fuzzy_score("界界", "界").is_some());
    }
}
