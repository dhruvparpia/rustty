//! TOML configuration (SPEC §13 schema, §14 defaults).
//!
//! User file: `%APPDATA%\rustty\config.toml` on Windows; in portable
//! mode (see [`crate::portable`]) the `config.toml` beside the executable
//! wins (CONFIG-020). Missing or invalid files fall back to defaults;
//! invalid values are reported by [`Config::load`]. Value ranges are
//! validated on parse (CONFIG-001..006): out-of-range values are errors,
//! not silent clamps.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use serde::Deserialize;

/// Default font family (SPEC §13 `font`).
pub const DEFAULT_FONT: &str = "Cascadia Mono";

/// Default font size in points (SPEC §13 `font-size`).
pub const DEFAULT_FONT_SIZE: f32 = 11.0;

/// Default line height as a fraction of the glyph box (SPEC §13
/// `line-height`).
pub const DEFAULT_LINE_HEIGHT: f32 = 1.2;

/// Default theme preset name (SPEC §14).
pub const DEFAULT_THEME: &str = "copper-dark";

/// Default scrollback depth (matches [`Grid::new`](crate::grid::Grid::new)).
pub const DEFAULT_SCROLLBACK: usize = 1000;

/// Smallest accepted `font-size` in points.
pub const MIN_FONT_SIZE: f32 = 5.0;

/// Largest accepted `font-size` in points.
pub const MAX_FONT_SIZE: f32 = 72.0;

/// Smallest accepted `line-height`.
pub const MIN_LINE_HEIGHT: f32 = 0.5;

/// Largest accepted `line-height`.
pub const MAX_LINE_HEIGHT: f32 = 4.0;

/// Largest accepted `scrollback` depth in lines.
pub const MAX_SCROLLBACK: usize = 1_000_000;

/// Largest accepted `[window] cols`.
pub const MAX_COLS: usize = 4000;

/// Largest accepted `[window] rows`.
pub const MAX_ROWS: usize = 1000;

/// Tab-bar visibility (SPEC §13 `tab-bar`, TAB-010 "hide-tab-bar setting").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TabBar {
    /// Always show the tab bar (default).
    #[default]
    Always,
    /// Show the tab bar only when more than one tab is open.
    Auto,
    /// Never show the tab bar (tabs switch with keybindings).
    Hidden,
}

/// Tab-bar placement (TAB-022).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TabBarPlacement {
    /// Put the strip above the grid (default).
    #[default]
    Top,
    /// Put the strip below the grid.
    Bottom,
}

/// How a BEL is announced (BELL-002/006/007/008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BellMode {
    /// Show nothing, make no sound (BELL-006).
    Silent,
    /// Flash the tab and the pane (BELL-003/004/007).
    Visual,
    /// Play the system bell (BELL-008).
    #[default]
    Audible,
    /// Flash and play the system bell.
    Both,
}

impl BellMode {
    /// Should a BEL flash the UI?
    #[must_use]
    pub fn visual(self) -> bool {
        matches!(self, Self::Visual | Self::Both)
    }

    /// Should a BEL play a sound?
    #[must_use]
    pub fn audible(self) -> bool {
        matches!(self, Self::Audible | Self::Both)
    }
}

/// Cursor shape (CONFIG-010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CursorShape {
    /// Filled block cursor.
    #[default]
    Block,
    /// Vertical bar cursor.
    Bar,
    /// Horizontal underline cursor.
    Underline,
}

/// Tab-bar settings (`[tab-bar]`, TAB-021..024, CONFIG-012).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabBarSection {
    /// Strip visibility.
    #[serde(default)]
    pub visible: TabBar,
    /// Strip placement.
    #[serde(default)]
    pub placement: TabBarPlacement,
    /// Show profile indicators in tabs (TAB-024).
    #[serde(rename = "show-profiles", default = "default_true")]
    pub show_profiles: bool,
    /// Show working directory in tab titles, when shell integration provides it.
    #[serde(rename = "show-cwd", default)]
    pub show_cwd: bool,
    /// Draw a close affordance on tabs.
    #[serde(rename = "close-button", default = "default_true")]
    pub close_button: bool,
}

impl Default for TabBarSection {
    fn default() -> Self {
        Self {
            visible: TabBar::default(),
            placement: TabBarPlacement::default(),
            show_profiles: true,
            show_cwd: false,
            close_button: true,
        }
    }
}

fn deserialize_tab_bar<'de, D>(deserializer: D) -> Result<TabBarSection, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    let value = toml::Value::deserialize(deserializer)?;
    match value {
        toml::Value::String(s) => {
            let visible = TabBar::deserialize(toml::Value::String(s)).map_err(D::Error::custom)?;
            Ok(TabBarSection {
                visible,
                ..TabBarSection::default()
            })
        }
        other => TabBarSection::deserialize(other).map_err(D::Error::custom),
    }
}

fn default_true() -> bool {
    true
}

/// Font settings as the renderer consumes them (SPEC FONT-002/008).
///
/// Built from the top-level `font`/`font-size`/`line-height` keys of
/// [`Config`] by [`Config::font`].
#[derive(Debug, Clone)]
pub struct FontConfig {
    /// Preferred family; missing families fall back through the
    /// Cascadia/Consolas chain (FONT-018).
    pub family: String,
    /// Face size in points.
    pub size: f32,
    /// Cell height as a fraction of the measured glyph box.
    pub line_height: f32,
    /// Font feature string (FONT-011, e.g. `"calt off"`).
    pub features: String,
    /// Ligatures on (FONT-010/011). The renderer shapes runs through
    /// DirectWrite while this is true; the per-cell glyph atlas cannot form a
    /// ligature, so it is bypassed.
    pub ligatures: bool,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: String::from(DEFAULT_FONT),
            size: DEFAULT_FONT_SIZE,
            line_height: DEFAULT_LINE_HEIGHT,
            features: String::new(),
            ligatures: true,
        }
    }
}

// f32 fields cannot derive Eq; compare floats with `total_cmp`, which is a
// total order (so the resulting relation is reflexive, symmetric, transitive).
impl PartialEq for FontConfig {
    fn eq(&self, other: &Self) -> bool {
        self.family == other.family
            && self.size.total_cmp(&other.size) == Ordering::Equal
            && self.line_height.total_cmp(&other.line_height) == Ordering::Equal
            && self.features == other.features
            && self.ligatures == other.ligatures
    }
}

impl Eq for FontConfig {}

/// Window/geometry settings (`[window]` section).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowConfig {
    /// Initial width in columns (default 80).
    #[serde(default = "default_cols")]
    pub cols: usize,
    /// Initial height in rows (default 24).
    #[serde(default = "default_rows")]
    pub rows: usize,
    /// Horizontal/vertical padding in pixels (WINDOW-011).
    #[serde(default)]
    pub padding: Padding,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            cols: default_cols(),
            rows: default_rows(),
            padding: Padding::default(),
        }
    }
}

fn default_cols() -> usize {
    80
}

/// Default rows (matches [`Grid::new`](crate::grid::Grid::new)).
fn default_rows() -> usize {
    24
}

/// Window padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Padding {
    /// Left/right padding in pixels.
    #[serde(default)]
    pub x: u32,
    /// Top/bottom padding in pixels.
    #[serde(default)]
    pub y: u32,
}

impl Default for Padding {
    fn default() -> Self {
        Self { x: 12, y: 10 }
    }
}

/// Split settings (`[split]`, CONFIG-013).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitConfig {
    /// Draw split borders.
    #[serde(default = "default_true")]
    pub visible: bool,
    /// Split border width in pixels.
    #[serde(default = "default_split_padding")]
    pub padding: u32,
    /// Draw split borders (alias of `visible`).
    #[serde(default = "default_true")]
    pub border: bool,
}

impl Default for SplitConfig {
    fn default() -> Self {
        Self {
            visible: true,
            padding: default_split_padding(),
            border: true,
        }
    }
}

fn default_split_padding() -> u32 {
    1
}

/// Search defaults (`[search]`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchConfig {
    /// Show the search UI.
    #[serde(default = "default_true")]
    pub visible: bool,
    /// Case-sensitive default.
    #[serde(rename = "case-sensitive-default", default)]
    pub case_sensitive_default: bool,
    /// Whole-word default.
    #[serde(rename = "whole-word-default", default)]
    pub whole_word_default: bool,
    /// Regex default.
    #[serde(rename = "regex-default", default)]
    pub regex_default: bool,
    /// SEARCH-023 (issue #53): lines of context shown above and below the
    /// current match in the result view. Clamped to 0..=16 at use.
    #[serde(rename = "context-lines", default = "default_context_lines")]
    pub context_lines: usize,
}

fn default_context_lines() -> usize {
    3
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            visible: true,
            case_sensitive_default: false,
            whole_word_default: false,
            regex_default: false,
            context_lines: default_context_lines(),
        }
    }
}

/// Which screen edge the quick terminal docks to (roadmap v0.2 "Hotkey
/// summon from screen edge").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuickEdge {
    /// Drop down from the top edge (default).
    #[default]
    Top,
    /// Rise from the bottom edge.
    Bottom,
}

/// Quick terminal (`[quick-terminal]`): a drop-down shell summoned from any
/// app by a system-wide hotkey (roadmap v0.2 "Hotkey summon from screen
/// edge"). It is a second window that hides instead of closing.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuickTerminalConfig {
    /// Register the global hotkey. Off by default: a system-wide hotkey
    /// is invasive, so it is opt-in.
    #[serde(default)]
    pub enabled: bool,
    /// The summon key, e.g. `"ctrl+grave"`. Ctrl/Alt/Shift only (a system
    /// hotkey needs a non-modifier key; Win is rejected).
    #[serde(default = "default_quick_key")]
    pub key: String,
    /// Screen edge to dock to: `top` or `bottom`.
    #[serde(default)]
    pub edge: QuickEdge,
    /// Grid rows when shown; the width spans the monitor's work area.
    #[serde(default = "default_quick_height")]
    pub height: usize,
    /// Hide when the window loses focus.
    #[serde(rename = "autohide", default = "default_true")]
    pub autohide: bool,
}

fn default_quick_key() -> String {
    String::from("ctrl+grave")
}

fn default_quick_height() -> usize {
    10
}

impl Default for QuickTerminalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            key: default_quick_key(),
            edge: QuickEdge::default(),
            height: default_quick_height(),
            autohide: true,
        }
    }
}

/// One configured shell profile (CONFIG-002, CONFIG-023, SHELL-012).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    /// Display/profile name.
    pub name: String,
    /// Program to launch.
    #[serde(default)]
    pub command: String,
    /// Arguments passed to the program.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment added to the shell process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Starting directory (`~`, `%USERPROFILE%`, absolute path, etc.).
    #[serde(rename = "starting-directory", default = "default_home")]
    pub starting_directory: String,
    /// Theme preset name.
    #[serde(default)]
    pub theme: String,
    /// Tab color (`#RRGGBB`).
    #[serde(rename = "tab-color", default)]
    pub tab_color: Option<String>,
    /// Optional profile font.
    #[serde(default)]
    pub font: Option<String>,
    /// Optional profile font size.
    #[serde(rename = "font-size", default)]
    pub font_size: Option<f32>,
    /// Optional profile line height.
    #[serde(rename = "line-height", default)]
    pub line_height: Option<f32>,
    /// Ask before closing tabs of this profile (TAB-018).
    #[serde(rename = "confirm-close")]
    pub confirm_close: Option<bool>,
    /// Optional profile scrollback.
    #[serde(default)]
    pub scrollback: Option<usize>,
}

fn default_home() -> String {
    String::from("~")
}

impl ProfileConfig {
    /// Theme preset, falling back to the top-level/global theme.
    #[must_use]
    pub fn theme_or<'a>(&'a self, global: &'a str) -> &'a str {
        if self.theme.is_empty() {
            global
        } else {
            &self.theme
        }
    }

    /// Font bundle merged with global defaults.
    #[must_use]
    pub fn font_with(&self, global: FontConfig) -> FontConfig {
        FontConfig {
            family: self.font.clone().unwrap_or(global.family),
            size: self.font_size.unwrap_or(global.size),
            line_height: self.line_height.unwrap_or(global.line_height),
            features: global.features,
            ligatures: global.ligatures,
        }
    }

    /// Starting directory, expanded for `~` and `%...%`.
    ///
    /// # Errors
    ///
    /// Returns an error when a path variable cannot be resolved.
    pub fn expanded_dir(&self) -> anyhow::Result<String> {
        Ok(expand_path(&self.starting_directory))
    }
}

/// Expand `~`, `%VAR%`, and Windows path variables.
#[must_use]
pub fn expand_path(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    if rest == "~" || rest.starts_with("~/") || rest.starts_with("~\\") {
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        out.push_str(&home);
        rest = rest.get(1..).unwrap_or("");
    }
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        if let Some(end) = tail.find('%') {
            let var = &tail[..end];
            out.push_str(&std::env::var(var).unwrap_or_default());
            rest = &tail[end + 1..];
        } else {
            out.push('%');
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// Key binding action executed by the window layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Open a new tab.
    NewTab,
    /// Close the active pane, then tab.
    CloseTab,
    /// Switch to the next tab.
    NextTab,
    /// Switch to the previous tab.
    PrevTab,
    /// Copy selection/screen.
    Copy,
    /// Paste clipboard.
    Paste,
    /// Toggle search.
    Search,
    /// Split active pane right.
    SplitRight,
    /// Split active pane below.
    SplitDown,
    /// Toggle fullscreen.
    Fullscreen,
    /// Quit.
    Quit,
    /// Focus the pane above.
    FocusUp,
    /// Focus the pane below.
    FocusDown,
    /// Focus the pane left.
    FocusLeft,
    /// Focus the pane right.
    FocusRight,
    /// Grow the focused pane above.
    ResizeUp,
    /// Grow the focused pane below.
    ResizeDown,
    /// Grow the focused pane left.
    ResizeLeft,
    /// Grow the focused pane right.
    ResizeRight,
    /// Select the previous profile.
    PrevProfile,
    /// Select the next profile.
    NextProfile,
    /// Select the previous theme.
    PrevTheme,
    /// Select the next theme.
    NextTheme,
    /// Move the active tab left.
    MoveTabLeft,
    /// Move the active tab right.
    MoveTabRight,
    /// Rename the active tab.
    RenameTab,
    /// Hide the quick terminal (no effect in a normal window).
    CloseQuick,
    /// Grow the font one step (A11Y "adjustable font size").
    IncreaseFontSize,
    /// Shrink the font one step.
    DecreaseFontSize,
    /// Return the font to the configured size.
    ResetFontSize,
    /// Re-read the config file now (CONFIG-018 on demand).
    ReloadConfig,
    /// Clear the screen: send Ctrl-L (TTY-016).
    ClearScreen,
    /// Scroll to the previous command boundary (SHELLINT-022, issue #38).
    PrevCommand,
    /// Scroll to the next command boundary (SHELLINT-022, issue #38).
    NextCommand,
    /// Copy the output of the last finished command (SHELLINT-023).
    CopyLastCommand,
    /// Open the command palette (issue #39).
    CommandPalette,
    /// Open keyboard hint mode (issue #52).
    HintMode,
    /// Add/remove a bookmark at the cursor or current search match
    /// (issue #53, BKMK-001).
    ToggleBookmark,
    /// Show the bookmark list (issue #53, BKMK-002).
    ShowBookmarks,
    /// Show or hide the sessions sidebar (RAIL-001).
    ToggleSessionRail,
    /// Show or hide the project's roadmap board (BOARD-001).
    ToggleBoard,
    /// Open a new session in its own Git worktree (WORKTREE-010).
    NewWorktreeSession,
    /// Create any missing project brain files (BRAIN-001).
    InitProjectBrain,
    /// Start a Boss Agent session for this project (BOSS-001).
    StartBoss,
    /// Export the current selection to a file (issue #53, EXPORT-001).
    ExportSelection,
    /// Export the last finished command's output to a file
    /// (issue #53, EXPORT-002).
    ExportLastCommand,
}

impl KeyAction {
    /// Parse a TOML action string.
    pub fn parse(action: &str) -> Option<Self> {
        let action = action.trim().replace('_', "-").to_ascii_lowercase();
        Some(match action.as_str() {
            "new-tab" | "newtab" => Self::NewTab,
            "close-tab" | "closetab" | "close" => Self::CloseTab,
            "next-tab" | "nexttab" => Self::NextTab,
            "prev-tab" | "previous-tab" | "prevtab" => Self::PrevTab,
            "copy" | "copy-selection" => Self::Copy,
            "paste" => Self::Paste,
            "search" | "toggle-search" => Self::Search,
            "split-right" | "split-right-pane" => Self::SplitRight,
            "split-down" | "split-below" => Self::SplitDown,
            "fullscreen" | "toggle-fullscreen" => Self::Fullscreen,
            "quit" => Self::Quit,
            "focus-up" => Self::FocusUp,
            "focus-down" => Self::FocusDown,
            "focus-left" => Self::FocusLeft,
            "focus-right" => Self::FocusRight,
            "resize-up" => Self::ResizeUp,
            "resize-down" => Self::ResizeDown,
            "resize-left" => Self::ResizeLeft,
            "resize-right" => Self::ResizeRight,
            "next-profile" => Self::NextProfile,
            "prev-profile" | "previous-profile" => Self::PrevProfile,
            "next-theme" => Self::NextTheme,
            "prev-theme" | "previous-theme" => Self::PrevTheme,
            "move-tab-left" | "move-tab-leftward" => Self::MoveTabLeft,
            "move-tab-right" => Self::MoveTabRight,
            "rename-tab" | "rename" => Self::RenameTab,
            "close-quick" | "hide-quick" => Self::CloseQuick,
            "increase-font-size" => Self::IncreaseFontSize,
            "decrease-font-size" => Self::DecreaseFontSize,
            "reset-font-size" => Self::ResetFontSize,
            "reload-config" => Self::ReloadConfig,
            "clear-screen" => Self::ClearScreen,
            "prev-command" | "previous-command" => Self::PrevCommand,
            "next-command" => Self::NextCommand,
            "copy-last-command" | "copy-command-output" => Self::CopyLastCommand,
            "command-palette" | "palette" => Self::CommandPalette,
            "hint-mode" | "hints" => Self::HintMode,
            "toggle-bookmark" | "bookmark" => Self::ToggleBookmark,
            "toggle-session-rail" | "session-rail" => Self::ToggleSessionRail,
            "toggle-board" | "board" => Self::ToggleBoard,
            "new-worktree-session" => Self::NewWorktreeSession,
            "init-project-brain" => Self::InitProjectBrain,
            "start-boss" => Self::StartBoss,
            "show-bookmarks" | "bookmarks" => Self::ShowBookmarks,
            "export-selection" => Self::ExportSelection,
            "export-last-command" | "export-command-output" => Self::ExportLastCommand,
            _ => return None,
        })
    }
}

/// Normalize a keybinding spec (`Ctrl+Shift+T` -> `ctrl+shift+t`).
#[must_use]
pub fn normalize_key_spec(spec: &str) -> String {
    spec.split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let lower = part.to_ascii_lowercase();
            match lower.as_str() {
                "ctrl" | "control" => "ctrl".to_string(),
                "shift" => "shift".to_string(),
                "alt" | "option" => "alt".to_string(),
                "super" | "cmd" | "win" | "windows" => "super".to_string(),
                "enter" | "return" | "cr" => "enter".to_string(),
                "esc" | "escape" => "escape".to_string(),
                "space" => "space".to_string(),
                "pgup" | "pageup" => "pageup".to_string(),
                "pgdn" | "pagedown" => "pagedown".to_string(),
                _ => lower,
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Parse a global hotkey spec for the quick terminal (`Ctrl+Grave` ->
/// `ctrl+grave`).
///
/// The spec must carry at least one of Ctrl/Alt/Shift: every system hotkey
/// is published to every app, so a plain key (or a Win-key combo) is
/// rejected. `grave` is the `` ` ``/`~` key.
///
/// # Errors
///
/// Names why the spec was rejected.
pub fn parse_hotkey(spec: &str) -> Result<String, String> {
    let normalized = normalize_key_spec(spec);
    let parts: Vec<&str> = normalized.split('+').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return Err("a hotkey needs a modifier (ctrl, alt, shift) and a key".to_string());
    }
    let key = parts[parts.len() - 1];
    let mods = &parts[..parts.len() - 1];
    if mods.contains(&"super") {
        return Err(format!(
            "the hotkey `{normalized}` uses the Super/Win key, which is not allowed"
        ));
    }
    if !mods.iter().any(|m| matches!(*m, "ctrl" | "alt" | "shift")) {
        return Err(format!(
            "the hotkey `{normalized}` needs a ctrl, alt or shift modifier"
        ));
    }
    let named = matches!(
        key,
        "enter"
            | "escape"
            | "space"
            | "tab"
            | "backspace"
            | "insert"
            | "delete"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "left"
            | "right"
            | "up"
            | "down"
            | "minus"
            | "equal"
            | "plus"
            | "grave"
            | "comma"
            | "period"
            | "slash"
            | "left-bracket"
            | "right-bracket"
            | "backslash"
            | "semicolon"
            | "quote"
    ) || (key.len() == 1
        && key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c.is_ascii_digit()))
        || (key.len() > 1
            && key.starts_with('f')
            && key[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)));
    if !named {
        return Err(format!(
            "the hotkey `{normalized}` ends in `{key}`, which is not a key name"
        ));
    }
    Ok(normalized)
}

/// The `theme` key: a preset/theme-file name, or a `[theme]` table of colour
/// overrides (CONFIG-002/003, THEME-009).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ThemeSetting {
    /// Preset name, or the name of `themes/<name>.toml`.
    Name(String),
    /// `[theme]` table; `name` inside it picks the base theme.
    Table(Box<crate::theme::ThemeFile>),
}

impl ThemeSetting {
    /// Name of the base theme (`""` when the table names none).
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Name(name) => name,
            Self::Table(file) => file.name.as_deref().unwrap_or(""),
        }
    }

    /// Colour overrides the key carries (empty for the string form).
    #[must_use]
    pub fn overrides(&self) -> crate::theme::ThemeFile {
        match self {
            Self::Name(_) => crate::theme::ThemeFile::default(),
            Self::Table(file) => file.as_ref().clone(),
        }
    }

    /// Point at a theme by name (theme cycling, `next-theme`).
    pub fn set_name(&mut self, name: &str) {
        *self = Self::Name(name.to_string());
    }
}

/// The full configuration tree (SPEC §13 top-level fields).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Theme: preset name, `themes/<name>.toml`, and/or colour overrides.
    #[serde(default = "default_theme_setting")]
    pub theme: ThemeSetting,
    /// Font family (SPEC `font`).
    #[serde(default = "default_font")]
    pub font: String,
    /// Font size in points (SPEC `font-size`).
    #[serde(rename = "font-size", default = "default_font_size")]
    pub font_size: f32,
    /// Cell line height (SPEC `line-height`).
    #[serde(rename = "line-height", default = "default_line_height")]
    pub line_height: f32,
    /// Scrollback depth in lines (SPEC `scrollback`).
    #[serde(default = "default_scrollback")]
    pub scrollback: usize,
    /// Tab-bar visibility and tab-bar section settings.
    #[serde(rename = "tab-bar", default, deserialize_with = "deserialize_tab_bar")]
    pub tab_bar: TabBarSection,
    /// Font ligatures (FONT-011).
    #[serde(default = "default_true")]
    pub ligatures: bool,
    /// Font feature string (FONT-011).
    #[serde(rename = "font-features", default)]
    pub font_features: String,
    /// Window opacity, 1..100 (WINDOW-010, CONFIG-011).
    #[serde(default = "default_opacity")]
    pub opacity: u8,
    /// Cursor shape (CONFIG-010).
    #[serde(rename = "cursor-shape", default)]
    pub cursor_shape: CursorShape,
    /// Cursor blink (CONFIG-009).
    #[serde(rename = "cursor-blink", default = "default_true")]
    pub cursor_blink: bool,
    /// Copy mouse selection immediately (CONFIG-008).
    #[serde(rename = "copy-on-select", default)]
    pub copy_on_select: bool,
    /// How a BEL is announced: `silent`, `visual`, `audible`, `both`.
    #[serde(default)]
    pub bell: BellMode,
    /// Save tabs, splits and the window frame on exit and restore them at
    /// startup (TAB-026, WINDOW-015/016).
    #[serde(rename = "session-restore", default = "default_true")]
    pub session_restore: bool,
    /// RAIL-001: show the sessions sidebar.
    #[serde(rename = "session-rail", default = "default_true")]
    pub session_rail: bool,
    /// AUTO-007: let any local client of this user open tabs and type into
    /// sessions. Off, writes need this window's token. A window reads it once, when
    /// it starts: a reload never changes what a running window grants.
    #[serde(rename = "automation-writes", default)]
    pub automation_writes: bool,
    /// WELCOME-001: show the crab banner when rustty starts.
    #[serde(default = "default_true")]
    pub welcome: bool,
    /// SHELLINT-013/014: launch known shells (Git Bash, PowerShell, WSL
    /// bash) with the rustty integration script, and enable per-command
    /// tracking, navigation and completion notices. Explicit opt-in.
    #[serde(rename = "shell-integration", default)]
    pub shell_integration: bool,
    /// SHELLINT-024: a finished command at least this many milliseconds
    /// long earns a completion toast while the window is in the background;
    /// 0 disables the toasts.
    #[serde(rename = "command-notify-ms", default = "default_notify_ms")]
    pub command_notify_ms: u64,
    /// Detach prefix for `rustty --attach` (issue #47): `C-<char>`,
    /// default `C-]` (Ctrl+]). `prefix d` detaches, `prefix ?` shows help.
    #[serde(rename = "detach-prefix", default = "default_detach_prefix")]
    pub detach_prefix: String,
    /// Command that opens diagnostic `file:line:col` references (issue
    /// #51): `{file}` `{line}` `{column}` placeholders, whitespace-split
    /// into argv; empty disables launching (copy-path only).
    #[serde(default = "default_editor")]
    pub editor: String,
    /// Profile used for the first tab.
    #[serde(rename = "starting-profile", default)]
    pub starting_profile: String,
    /// `[window]` section.
    #[serde(default)]
    pub window: WindowConfig,
    /// `[split]` section.
    #[serde(default)]
    pub split: SplitConfig,
    /// `[search]` section.
    #[serde(default)]
    pub search: SearchConfig,
    /// `[quick-terminal]` section (roadmap v0.2 hotkey summon).
    #[serde(rename = "quick-terminal", default)]
    pub quick_terminal: QuickTerminalConfig,
    /// `[keybindings]` table: normalized key string to action.
    #[serde(default)]
    pub keybindings: BTreeMap<String, String>,
    /// TOML profiles (`[[profile]]`, SPEC spells it singular).
    #[serde(rename = "profile", alias = "profiles", default)]
    pub profiles: Vec<ProfileConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: default_theme_setting(),
            font: default_font(),
            font_size: default_font_size(),
            line_height: default_line_height(),
            scrollback: default_scrollback(),
            tab_bar: TabBarSection::default(),
            ligatures: true,
            font_features: String::new(),
            opacity: default_opacity(),
            cursor_shape: CursorShape::default(),
            cursor_blink: true,
            copy_on_select: false,
            bell: BellMode::default(),
            session_restore: true,
            welcome: true,
            session_rail: true,
            automation_writes: false,
            shell_integration: false,
            command_notify_ms: default_notify_ms(),
            detach_prefix: default_detach_prefix(),
            editor: default_editor(),
            starting_profile: String::new(),
            window: WindowConfig::default(),
            split: SplitConfig::default(),
            search: SearchConfig::default(),
            quick_terminal: QuickTerminalConfig::default(),
            keybindings: BTreeMap::new(),
            profiles: Vec::new(),
        }
    }
}

// `font_size`/`line_height` are f32: derive(Eq) is impossible, so compare
// floats with `total_cmp` (see the FontConfig note above).
impl PartialEq for Config {
    fn eq(&self, other: &Self) -> bool {
        self.theme == other.theme
            && self.font == other.font
            && self.font_size.total_cmp(&other.font_size) == Ordering::Equal
            && self.line_height.total_cmp(&other.line_height) == Ordering::Equal
            && self.font_features == other.font_features
            && self.scrollback == other.scrollback
            && self.tab_bar == other.tab_bar
            && self.ligatures == other.ligatures
            && self.font_features == other.font_features
            && self.opacity == other.opacity
            && self.cursor_shape == other.cursor_shape
            && self.cursor_blink == other.cursor_blink
            && self.copy_on_select == other.copy_on_select
            && self.session_restore == other.session_restore
            && self.welcome == other.welcome
            && self.session_rail == other.session_rail
            && self.automation_writes == other.automation_writes
            && self.shell_integration == other.shell_integration
            && self.command_notify_ms == other.command_notify_ms
            && self.detach_prefix == other.detach_prefix
            && self.editor == other.editor
            && self.starting_profile == other.starting_profile
            && self.window == other.window
            && self.split == other.split
            && self.search == other.search
            && self.quick_terminal == other.quick_terminal
            && self.keybindings == other.keybindings
            && self.profiles == other.profiles
    }
}

impl Eq for Config {}

/// The default theme name is empty: follow the system dark/light setting
/// (THEME-010). A name in the config wins over the system.
fn default_theme_setting() -> ThemeSetting {
    ThemeSetting::Name(String::new())
}

fn default_font() -> String {
    String::from(DEFAULT_FONT)
}

fn default_font_size() -> f32 {
    DEFAULT_FONT_SIZE
}

fn default_line_height() -> f32 {
    DEFAULT_LINE_HEIGHT
}

fn default_scrollback() -> usize {
    DEFAULT_SCROLLBACK
}

fn default_opacity() -> u8 {
    100
}

fn default_notify_ms() -> u64 {
    10_000
}

/// Default detach prefix for the attach frontend (issue #47): Ctrl+].
fn default_detach_prefix() -> String {
    "C-]".to_string()
}

/// Default editor command for diagnostic references (issue #51).
fn default_editor() -> String {
    "code --goto {file}:{line}:{column}".to_string()
}

fn validate_keybindings(map: &BTreeMap<String, String>) -> anyhow::Result<()> {
    for (key, action) in map {
        let key = normalize_key_spec(key);
        if key.is_empty() {
            anyhow::bail!("keybindings: key must not be empty");
        }
        if key.split('+').any(|part| part == "super") {
            // The dispatcher matches Ctrl/Shift/Alt only, so a Win-key binding
            // would be accepted and then never fire (CONFIG-011).
            anyhow::bail!("keybindings: {key}: the Super/Win key cannot be bound");
        }
        if KeyAction::parse(action).is_none() {
            anyhow::bail!("keybindings: unknown action {action}");
        }
    }
    Ok(())
}

fn font_feature_string(ligatures: bool, features: &str) -> String {
    // The ligature switches are sent explicitly: DirectWrite shaping then
    // turns the font's ligature features on deterministically instead of
    // relying on its defaults (FONT-010). `font-features` goes last, so a
    // hand-written tag overrides the switch.
    let mut out = String::from(if ligatures {
        "calt 1 liga 1 clig 1"
    } else {
        "calt 0 liga 0 clig 0 dlig 0"
    });
    if !features.trim().is_empty() {
        out.push(' ');
        out.push_str(features.trim());
    }
    out
}

fn parse_hex_color(text: &str) -> Result<u32, String> {
    let text = text.trim();
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("expected #RRGGBB, got {text}"));
    }
    u32::from_str_radix(hex, 16).map_err(|e| e.to_string())
}

impl Config {
    /// The font settings for the renderer, as one bundle.
    #[must_use]
    pub fn font(&self) -> FontConfig {
        FontConfig {
            family: self.font.clone(),
            size: self.font_size,
            line_height: self.line_height,
            features: font_feature_string(self.ligatures, &self.font_features),
            ligatures: self.ligatures,
        }
    }

    /// Parsed keybinding map for window dispatch.
    #[must_use]
    pub fn keymap(&self) -> BTreeMap<String, KeyAction> {
        self.keybindings
            .iter()
            .filter_map(|(key, action)| {
                let key = normalize_key_spec(key);
                KeyAction::parse(action).map(|a| (key, a))
            })
            .collect()
    }

    /// Lookup a profile by name.
    #[must_use]
    pub fn profile(&self, name: &str) -> Option<&ProfileConfig> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// Parse a configuration string and validate every value (THEME-013
    /// validates on load; out-of-range values are rejected, not clamped).
    ///
    /// # Errors
    ///
    /// Returns an error when the TOML is malformed, a value has the wrong
    /// type, an unknown key appears, or a value is out of its accepted range.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let cfg: Self = toml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Check every value against its accepted range.
    ///
    /// # Errors
    ///
    /// Names the offending key and its accepted range.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.font.trim().is_empty() {
            anyhow::bail!("font: family must not be empty");
        }
        if !(MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&self.font_size) {
            anyhow::bail!("font-size: must be between {MIN_FONT_SIZE} and {MAX_FONT_SIZE} points");
        }
        if !(MIN_LINE_HEIGHT..=MAX_LINE_HEIGHT).contains(&self.line_height) {
            anyhow::bail!("line-height: must be between {MIN_LINE_HEIGHT} and {MAX_LINE_HEIGHT}");
        }
        if self.scrollback > MAX_SCROLLBACK {
            anyhow::bail!("scrollback: must be at most {MAX_SCROLLBACK} lines");
        }
        if self.window.cols == 0 || self.window.cols > MAX_COLS {
            anyhow::bail!("window.cols: must be between 1 and {MAX_COLS}");
        }
        if self.window.rows == 0 || self.window.rows > MAX_ROWS {
            anyhow::bail!("window.rows: must be between 1 and {MAX_ROWS}");
        }
        if self.window.padding.x > 1000 || self.window.padding.y > 1000 {
            anyhow::bail!("window.padding: must be at most 1000 pixels");
        }
        if self.opacity == 0 || self.opacity > 100 {
            anyhow::bail!("opacity: must be between 1 and 100");
        }
        // detach-prefix must decode to a control byte ("C-<ascii char>").
        // The parser lives in the Windows-only attach frontend.
        #[cfg(windows)]
        crate::attach::parse_prefix(&self.detach_prefix)
            .map_err(|e| anyhow::anyhow!("detach-prefix: {e}"))?;
        if self.split.padding > 1000 {
            anyhow::bail!("split.padding: must be at most 1000 pixels");
        }
        validate_keybindings(&self.keybindings)?;
        if self.quick_terminal.height == 0 || self.quick_terminal.height > MAX_ROWS {
            anyhow::bail!("quick-terminal.height: must be between 1 and {MAX_ROWS}");
        }
        if self.quick_terminal.enabled {
            // A disabled hotkey registers nothing, so its key is not checked;
            // an enabled one must name a key the system will deliver.
            parse_hotkey(&self.quick_terminal.key)
                .map_err(|e| anyhow::anyhow!("quick-terminal.key: {e}"))?;
        }
        for (idx, profile) in self.profiles.iter().enumerate() {
            if profile.name.trim().is_empty() {
                anyhow::bail!("profiles[{idx}].name: must not be empty");
            }
            if profile.command.trim().is_empty() {
                anyhow::bail!("profiles[{idx}].command: must not be empty");
            }
            if let Some(size) = profile.font_size
                && !(MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&size)
            {
                anyhow::bail!(
                    "profiles[{idx}].font-size: must be between {MIN_FONT_SIZE} and {MAX_FONT_SIZE} points"
                );
            }
            if let Some(height) = profile.line_height
                && !(MIN_LINE_HEIGHT..=MAX_LINE_HEIGHT).contains(&height)
            {
                anyhow::bail!(
                    "profiles[{idx}].line-height: must be between {MIN_LINE_HEIGHT} and {MAX_LINE_HEIGHT}"
                );
            }
            if let Some(sb) = profile.scrollback
                && sb > MAX_SCROLLBACK
            {
                anyhow::bail!("profiles[{idx}].scrollback: must be at most {MAX_SCROLLBACK} lines");
            }
            if let Some(color) = &profile.tab_color {
                parse_hex_color(color)
                    .map_err(|e| anyhow::anyhow!("profiles[{idx}].tab-color: {e}"))?;
            }
        }
        for name in self.profiles.iter().map(|p| &p.name) {
            if self.profiles.iter().filter(|p| &p.name == name).count() > 1 {
                anyhow::bail!("profiles: duplicate profile name {name}");
            }
        }
        Ok(())
    }

    /// Load from the user config file, falling back to defaults.
    ///
    /// A missing file is not an error. A present-but-invalid file is.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read, parsed,
    /// or validated.
    pub fn load() -> anyhow::Result<Self> {
        let Some(path) = config_path() else {
            return Ok(Self::default());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(anyhow::Error::new(e).context(format!("reading {}", path.display()))),
        }
    }
}

/// Path of the user config file, or `None` when `%APPDATA%` is unset.
///
/// Portable mode wins over `%APPDATA%` (CONFIG-020, PKG-004): when the
/// executable sits in a portable folder (see [`crate::portable`]), the
/// `config.toml` beside it is used. When the `rustty` dir has no config
/// yet and the pre-rename `rusttyterm` dir does, that file is read, so
/// the rename does not silently reset user settings.
#[must_use]
pub fn config_path() -> Option<PathBuf> {
    crate::portable::running_config_path().or_else(|| {
        std::env::var_os("APPDATA").map(|dir| {
            let dir = PathBuf::from(dir);
            let new = dir.join("rustty").join("config.toml");
            if !new.is_file() {
                let old = dir.join("rusttyterm").join("config.toml");
                if old.is_file() {
                    return old;
                }
            }
            new
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_uses_defaults() {
        let cfg = Config::parse("").expect_result();
        assert_eq!(cfg, Config::default());
        assert!(
            cfg.theme.name().is_empty(),
            "an unset theme follows the system (THEME-010)"
        );
        assert_eq!(cfg.font, DEFAULT_FONT);
        assert_eq!(cfg.font_size, DEFAULT_FONT_SIZE);
        assert_eq!(cfg.scrollback, DEFAULT_SCROLLBACK);
        assert_eq!(cfg.tab_bar.visible, TabBar::Always);
    }

    #[test]
    fn the_bell_mode_is_configurable_and_defaults_to_audible() {
        assert_eq!(Config::parse("").expect_result().bell, BellMode::Audible);
        for (text, mode) in [
            ("bell = \"silent\"", BellMode::Silent),
            ("bell = \"visual\"", BellMode::Visual),
            ("bell = \"audible\"", BellMode::Audible),
            ("bell = \"both\"", BellMode::Both),
        ] {
            assert_eq!(Config::parse(text).expect_result().bell, mode);
        }
        assert!(Config::parse("bell = \"loud\"").is_err());
        assert!(!BellMode::Silent.visual() && !BellMode::Silent.audible());
        assert!(BellMode::Visual.visual() && !BellMode::Visual.audible());
        assert!(BellMode::Audible.audible() && !BellMode::Audible.visual());
        assert!(BellMode::Both.visual() && BellMode::Both.audible());
    }

    #[test]
    fn theme_accepts_a_name_and_a_table_of_overrides() {
        let cfg = Config::parse(
            "theme = \"one-dark\"
",
        )
        .expect_result();
        assert_eq!(cfg.theme.name(), "one-dark");
        assert!(cfg.theme.overrides().background.is_none());

        let cfg = Config::parse(
            "[theme]
name = \"gruvbox-dark\"
background = \"#000000\"
",
        )
        .expect_result();
        assert_eq!(cfg.theme.name(), "gruvbox-dark");
        assert_eq!(cfg.theme.overrides().background.as_deref(), Some("#000000"));
    }

    #[test]
    fn top_level_keys_parse() {
        let cfg = Config::parse(
            "theme = \"gruvbox-dark\"\nfont = \"Consolas\"\nfont-size = 14\nline-height = 1.4\nscrollback = 5000\ntab-bar = \"auto\"\n",
        )
        .expect_result();
        assert_eq!(cfg.theme.name(), "gruvbox-dark");
        assert_eq!(cfg.font, "Consolas");
        assert_eq!(cfg.font_size, 14.0);
        assert_eq!(cfg.line_height, 1.4);
        assert_eq!(cfg.scrollback, 5000);
        assert_eq!(cfg.tab_bar.visible, TabBar::Auto);
    }

    #[test]
    fn partial_keys_fill_defaults() {
        let cfg = Config::parse("[window]\ncols = 120\n").expect_result();
        assert_eq!(cfg.window.cols, 120);
        assert_eq!(cfg.window.rows, 24);
        assert_eq!(cfg.scrollback, DEFAULT_SCROLLBACK);
    }

    #[test]
    fn font_bundle_matches_top_level_keys() {
        let cfg = Config::parse("font = \"Courier New\"\nfont-size = 9.5\nline-height = 1.1\n")
            .expect_result();
        let font = cfg.font();
        assert_eq!(font.family, "Courier New");
        assert_eq!(font.size, 9.5);
        assert_eq!(font.line_height, 1.1);
    }

    #[test]
    fn ligatures_drive_the_feature_string() {
        let on = Config::parse("").expect_result().font();
        assert!(on.ligatures);
        assert!(on.features.contains("calt 1"), "got {:?}", on.features);

        let off = Config::parse("ligatures = false\n").expect_result().font();
        assert!(!off.ligatures);
        assert!(off.features.contains("calt 0"), "got {:?}", off.features);

        // A hand-written tag goes last, so it overrides the switch.
        let cfg = Config::parse("font-features = \"calt 0\"\n").expect_result();
        assert!(cfg.font().features.ends_with("calt 0"));
    }

    #[test]
    fn tab_bar_values_parse() {
        for (text, want) in [
            ("tab-bar = \"always\"", TabBar::Always),
            ("tab-bar = \"auto\"", TabBar::Auto),
            ("tab-bar = \"hidden\"", TabBar::Hidden),
        ] {
            assert_eq!(Config::parse(text).expect_result().tab_bar.visible, want);
        }
        assert!(Config::parse("tab-bar = \"sideways\"").is_err());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let res = Config::parse("[window]\nbanana = 1\n");
        assert!(res.is_err(), "unknown keys must fail");
        assert!(
            Config::parse("banana = 1").is_err(),
            "unknown top-level keys must fail"
        );
    }

    #[test]
    fn bad_type_is_rejected() {
        let res = Config::parse("[window]\ncols = \"wide\"\n");
        assert!(res.is_err(), "type errors must fail");
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        assert!(
            Config::parse("font-size = 3").is_err(),
            "below MIN_FONT_SIZE"
        );
        assert!(
            Config::parse("font-size = 80").is_err(),
            "above MAX_FONT_SIZE"
        );
        assert!(Config::parse("line-height = 0.1").is_err());
        assert!(Config::parse("line-height = 9").is_err());
        assert!(Config::parse("scrollback = 2000000").is_err());
        assert!(Config::parse("font = \"  \"").is_err(), "blank family");
        assert!(Config::parse("[window]\ncols = 0\n").is_err());
        assert!(Config::parse("[window]\nrows = 5000\n").is_err());
    }

    #[test]
    fn quick_terminal_defaults_to_off() {
        let cfg = Config::parse("").expect_result();
        assert!(!cfg.quick_terminal.enabled);
        assert_eq!(cfg.quick_terminal.key, "ctrl+grave");
        assert_eq!(cfg.quick_terminal.edge, QuickEdge::Top);
        assert_eq!(cfg.quick_terminal.height, 10);
        assert!(cfg.quick_terminal.autohide);
    }

    #[test]
    fn quick_terminal_section_parses_and_validates() {
        let cfg = Config::parse(
            "[quick-terminal]\nenabled = true\nkey = \"Ctrl+Alt+J\"\nedge = \"bottom\"\nheight = 14\nautohide = false\n",
        )
        .expect_result();
        assert!(cfg.quick_terminal.enabled);
        assert_eq!(cfg.quick_terminal.edge, QuickEdge::Bottom);
        assert_eq!(cfg.quick_terminal.height, 14);
        assert!(!cfg.quick_terminal.autohide);
        assert!(
            Config::parse("[quick-terminal]\nenabled = true\nkey = \"j\"\n").is_err(),
            "a system hotkey needs a modifier"
        );
        assert!(
            Config::parse("[quick-terminal]\nenabled = true\nkey = \"win+j\"\n").is_err(),
            "a system hotkey may not use the Win key"
        );
        assert!(
            Config::parse("[quick-terminal]\nheight = 0\n").is_err(),
            "a zero-row quick terminal is a typo"
        );
        // A disabled hotkey registers nothing, so its key is not validated.
        assert!(Config::parse("[quick-terminal]\nkey = \"banana\"\n").is_ok());
    }

    #[test]
    fn hotkey_specs_normalize_and_validate() {
        assert_eq!(parse_hotkey("Ctrl+Grave").expect_result(), "ctrl+grave");
        assert_eq!(
            parse_hotkey("ctrl + shift + F2").expect_result(),
            "ctrl+shift+f2"
        );
        assert!(
            parse_hotkey("ctrl+grave+shift").is_err(),
            "the key comes last"
        );
        assert!(parse_hotkey("alt+ctrl").is_err(), "two modifiers, no key");
    }

    #[test]
    fn key_action_strings() {
        // The v0.4 parity actions, plus Ghostty's underscore spelling.
        assert_eq!(
            KeyAction::parse("increase-font-size"),
            Some(KeyAction::IncreaseFontSize)
        );
        assert_eq!(
            KeyAction::parse("increase_font_size"),
            Some(KeyAction::IncreaseFontSize)
        );
        assert_eq!(
            KeyAction::parse("decrease-font-size"),
            Some(KeyAction::DecreaseFontSize)
        );
        assert_eq!(
            KeyAction::parse("reset-font-size"),
            Some(KeyAction::ResetFontSize)
        );
        assert_eq!(
            KeyAction::parse("reload-config"),
            Some(KeyAction::ReloadConfig)
        );
        assert_eq!(
            KeyAction::parse("clear-screen"),
            Some(KeyAction::ClearScreen)
        );
        assert_eq!(
            KeyAction::parse("shrink-wrap"),
            None,
            "unknown actions stay None"
        );
    }

    trait Expect<T> {
        fn expect_result(self) -> T;
    }

    impl<T> Expect<T> for Result<T, anyhow::Error> {
        fn expect_result(self) -> T {
            match self {
                Ok(v) => v,
                Err(e) => panic!("{e}"),
            }
        }
    }

    impl<T> Expect<T> for Result<T, String> {
        fn expect_result(self) -> T {
            match self {
                Ok(v) => v,
                Err(e) => panic!("{e}"),
            }
        }
    }

    #[test]
    fn default_padding_is_twelve_by_ten() {
        // Break caught: text flush against the window edges by default.
        assert_eq!(Padding::default(), Padding { x: 12, y: 10 });
        let cfg = Config::parse("").expect("empty config");
        assert_eq!(cfg.window.padding, Padding { x: 12, y: 10 });
    }

    #[test]
    fn explicit_zero_padding_is_honoured() {
        // Break caught: the new default overriding what the user set.
        let cfg = Config::parse("[window]\npadding = { x = 0, y = 0 }\n").expect("config");
        assert_eq!(cfg.window.padding, Padding { x: 0, y: 0 });
    }
}
