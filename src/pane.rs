//! Pane: the split leaf that holds one grid, one VT parser and one shell.
//!
//! Splits are out of MVP scope (spec §4), so a pane is a 1:1 owner of one
//! [`Grid`] and one [`Shell`] for now. The split tree in v0.3 will reuse
//! this type unchanged.
//!
//! The pane is the owner of the *screen* state the parser reports as
//! events: the alternate screen (TTY-018) is a main-grid/alt-grid swap the
//! pane performs, tab titles (OSC) become [`Pane::title`], and DSR/DA
//! replies go straight back to the shell. Scroll snap (returning to the
//! live edge when new output arrives) is applied here too.

use crate::grid::Grid;
use crate::parser::{Parser, ParserEvent};
use crate::selection::Selection;
use crate::shell::{Shell, ShellError, ShellEvent, SpawnOptions};

/// Turn an OSC 7 `file://host/path` (percent-encoded) into a path.
fn cwd_from_url(url: &str) -> String {
    let rest = url.strip_prefix("file://").unwrap_or(url);
    // Drop the host component (everything up to the first '/'), keeping a
    // leading slash for the absolute path that follows.
    let path = match rest.find('/') {
        Some(i) => &rest[i..],
        None => return String::new(),
    };
    let mut out = String::with_capacity(path.len());
    let mut chars = path.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2
                && let Ok(byte) = u8::from_str_radix(&hex, 16)
            {
                out.push(char::from(byte));
                continue;
            }
        }
        out.push(c);
    }
    // `/c/Users/x` and `/C:/Users/x` are both used; keep the drive form.
    let bytes: Vec<char> = out.chars().collect();
    if bytes.len() >= 3 && bytes[0] == '/' && bytes[2] == ':' && bytes[1].is_ascii_alphabetic() {
        return out[1..].to_string();
    }
    out
}

/// A single terminal pane (one grid, one parser, one shell process).
#[derive(Debug)]
pub struct Pane {
    grid: Grid,
    shell: Shell,
    parser: Parser,
    title: Option<String>,
    /// Last working directory the shell reported (OSC 7, SHELL-001).
    cwd: Option<String>,
    /// OSC 133;A/133;D seen: the shell is at a prompt.
    at_prompt: bool,
    /// OSC 133;B/C seen without a matching `D`: a command is running.
    command_running: bool,
    /// Exit code from the last `OSC 133;D:<code>`.
    last_exit: Option<i32>,
    /// Main screen kept while the alternate screen is active (TTY-018).
    main_grid: Option<Grid>,
    in_alt: bool,
    /// Grid changed since the renderer last asked (repaint hint).
    dirty: bool,
    /// Mouse text selection, in absolute grid coordinates (WIN-004).
    selection: Option<Selection>,
}

impl Pane {
    /// Create a pane with a fresh grid, a fresh parser and an inactive shell.
    #[must_use]
    pub fn new() -> Self {
        Self {
            grid: Grid::new(),
            shell: Shell::new(),
            parser: Parser::new(),
            title: None,
            cwd: None,
            at_prompt: false,
            command_running: false,
            last_exit: None,
            main_grid: None,
            in_alt: false,
            dirty: true,
            selection: None,
        }
    }

    /// Create a pane whose grid starts at `cols` x `rows` with a
    /// `scrollback`-line history (config `scrollback`).
    #[must_use]
    pub fn with_size(cols: usize, rows: usize, scrollback: usize) -> Self {
        Self {
            grid: Grid::with_size(cols, rows, scrollback),
            shell: Shell::new(),
            parser: Parser::new(),
            title: None,
            cwd: None,
            at_prompt: false,
            command_running: false,
            last_exit: None,
            main_grid: None,
            in_alt: false,
            dirty: true,
            selection: None,
        }
    }

    /// The pane's grid (viewport rows are `0..grid.rows`).
    #[must_use]
    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    /// The pane's grid, mutably (renderer/test use).
    #[must_use]
    pub fn grid_mut(&mut self) -> &mut Grid {
        &mut self.grid
    }

    /// Current mouse selection, if any (absolute grid coordinates).
    #[must_use]
    pub fn selection(&self) -> Option<Selection> {
        self.selection
    }

    /// Set the mouse selection (the renderer repaints the highlight).
    pub fn set_selection(&mut self, sel: Option<Selection>) {
        self.selection = sel;
        self.dirty = true;
    }

    /// Plain text of the current selection, `None` when nothing is
    /// selected (Ctrl+Shift+C copy source of truth).
    #[must_use]
    pub fn selection_text(&self) -> Option<String> {
        let sel = self.selection?;
        let text = self.grid.selection_text(&sel);
        (!text.is_empty()).then_some(text)
    }

    /// The pane's parser (cursor position, modes) for the renderer.
    #[must_use]
    pub fn parser(&self) -> &Parser {
        &self.parser
    }

    /// The pane's shell.
    #[must_use]
    pub fn shell(&self) -> &Shell {
        &self.shell
    }

    /// The pane's shell, mutably (input/resize/close go through this).
    #[must_use]
    pub fn shell_mut(&mut self) -> &mut Shell {
        &mut self.shell
    }

    /// Title the shell last set with OSC 0/1/2, if any (tab title source).
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Working directory the shell reported with OSC 7 (SHELL-001), if any.
    #[must_use]
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// SHELL-003: is the shell sitting at a prompt (after the first prompt
    /// marker)? TAB-014/021 and the notification plan use this.
    #[must_use]
    pub fn prompt_ready(&self) -> bool {
        self.at_prompt
    }

    /// Exit code of the last command that finished, from `OSC 133;D:<code>`.
    #[must_use]
    pub fn last_exit_code(&self) -> Option<i32> {
        self.last_exit
    }

    /// A command is running: between the prompt and `OSC 133;D`.
    #[must_use]
    pub fn command_running(&self) -> bool {
        self.command_running
    }

    /// Turn a `file://host/path` report into a path string.
    fn record_cwd(&mut self, url: &str) {
        self.cwd = Some(cwd_from_url(url));
    }

    /// Whether the alternate screen is active (TTY-018).
    #[must_use]
    pub const fn in_alt_screen(&self) -> bool {
        self.in_alt
    }

    /// Whether the cursor should be drawn (`CSI ?25`).
    #[must_use]
    pub const fn cursor_visible(&self) -> bool {
        self.parser.cursor_visible
    }

    /// Feed shell output bytes: parse into the grid, then apply the events
    /// the parser asks for (title, alt-screen swap, scrollback clear,
    /// DSR/DA replies) and snap the viewport back to the live edge when the
    /// bytes printed (`printed` in [`crate::parser::AdvanceOutcome`]).
    pub fn feed(&mut self, bytes: &[u8]) {
        self.dirty = true;
        let outcome = self.parser.advance(&mut self.grid, bytes);
        let cols = self.grid.cols;
        let rows = self.grid.rows;
        let scrollback = self.grid.scrollback();

        for event in outcome.events {
            match event {
                ParserEvent::Title(title) => self.title = Some(title),
                ParserEvent::Cwd(url) => self.record_cwd(&url),
                ParserEvent::PromptStart => {
                    self.at_prompt = true;
                    self.command_running = false;
                }
                ParserEvent::CommandStart | ParserEvent::CommandOutput => {
                    self.command_running = true;
                    self.at_prompt = false;
                }
                ParserEvent::PromptEnd(code) => {
                    self.at_prompt = true;
                    self.command_running = false;
                    self.last_exit = code;
                }
                ParserEvent::EnterAltScreen => {
                    if !self.in_alt {
                        let fresh = Grid::with_size(cols, rows, 0);
                        self.main_grid = Some(std::mem::replace(&mut self.grid, fresh));
                        self.in_alt = true;
                    }
                }
                ParserEvent::ExitAltScreen => {
                    if self.in_alt {
                        if let Some(main) = self.main_grid.take() {
                            self.grid = main;
                            self.grid.resize(cols, rows);
                        }
                        self.in_alt = false;
                    }
                }
                // The parser owns the flag; the renderer reads it via
                // `Pane::cursor_visible`, so there is nothing to apply.
                ParserEvent::CursorVisibility(_) => {}
                ParserEvent::ClearScrollback => {
                    self.grid = Grid::with_size(cols, rows, scrollback);
                }
                // DSR/DA replies go straight back to the shell.
                ParserEvent::Respond(reply) => {
                    let _ = self.shell.write_input(&reply);
                }
            }
        }

        if outcome.printed {
            let above = self.grid.scroll_offset();
            if above > 0 {
                self.grid.scroll_down(above);
            }
        }
        self.parser.fit(&self.grid);
    }

    /// Drain everything the shell reader thread reported, in order.
    ///
    /// Output chunks are fed to the parser; an exit event lands in
    /// [`Shell::state`] for the UI (TTY-025).
    pub fn drain_shell(&mut self) {
        for event in self.shell.poll_events() {
            match event {
                ShellEvent::Chunks(bytes) => self.feed(&bytes),
                ShellEvent::Exited(_) => {}
            }
        }
    }

    /// Start the pane's shell process (TTY-001, TTY-002).
    pub fn spawn_shell(&mut self, opts: &SpawnOptions) -> Result<(), ShellError> {
        self.shell.spawn(opts)
    }

    /// Send keyboard/paste bytes to the shell (TTY-004..TTY-017).
    pub fn send_input(&mut self, bytes: &[u8]) -> Result<(), ShellError> {
        self.shell.write_input(bytes)
    }

    /// Ask the shell to exit cleanly (TTY-024); the exit code arrives later
    /// through [`Pane::drain_shell`].
    pub fn close_shell(&mut self) {
        self.shell.close();
    }

    /// Resize the pane's grid, the saved main grid, the parser's bounds and
    /// the pseudo-console, in that order (spec §17: UI -> grid -> ConPTY ->
    /// shell). A ConPTY resize failure is reported, never a panic.
    ///
    /// The mouse selection is dropped: it is stored in absolute grid rows,
    /// and reflow moves those rows, so keeping it would highlight the wrong
    /// text (the resize repaint clears the highlight).
    pub fn resize(&mut self, cols: usize, rows: usize) -> Result<(), ShellError> {
        self.dirty = true;
        self.selection = None;
        self.grid.resize(cols, rows);
        if let Some(main) = &mut self.main_grid {
            main.resize(cols, rows);
        }
        self.parser.fit(&self.grid);
        self.shell.resize(cols, rows)
    }

    /// Take the repaint hint: `true` when the grid changed since the last
    /// call. The window layer invalidates its client area on `true` only,
    /// so an idle terminal costs no repaints (NFR-113/114).
    #[must_use]
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
}

impl Default for Pane {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::Pos;

    #[test]
    fn new_pane_is_inactive() {
        let pane = Pane::new();
        assert!(pane.shell().state() == crate::shell::ShellState::Inactive);
        assert_eq!(pane.grid().cols, 80);
        assert!(pane.title().is_none());
        assert!(!pane.in_alt_screen());
    }

    #[test]
    fn osc_7_and_133_update_the_cwd_prompt_state_and_exit_code() {
        let mut pane = Pane::with_size(20, 5, 10);
        pane.feed(b"\x1b]7;file://DESKTOP/C:/src/rust\x07");
        assert_eq!(pane.cwd(), Some("C:/src/rust"));
        pane.feed(b"\x1b]7;file://HOST/home/dhruv/one%20two\x07");
        assert_eq!(pane.cwd(), Some("/home/dhruv/one two"));
        assert!(!pane.prompt_ready());
        pane.feed(b"\x1b]133;A\x07");
        assert!(pane.prompt_ready());
        pane.feed(b"\x1b]133;B\x07\x1b]133;C\x07");
        assert!(pane.command_running() && !pane.prompt_ready());
        pane.feed(b"\x1b]133;D:2\x07");
        assert!(pane.prompt_ready() && !pane.command_running());
        assert_eq!(pane.last_exit_code(), Some(2));
    }

    #[test]
    fn feed_writes_text_and_applies_title() {
        let mut pane = Pane::new();
        pane.feed(b"hello\x1b]0;My Title\x07");
        assert_eq!(pane.grid().line_text(0).as_deref(), Some("hello"));
        assert_eq!(pane.title(), Some("My Title"));
    }

    #[test]
    fn alt_screen_swaps_and_restores_the_main_grid() {
        let mut pane = Pane::new();
        pane.feed(b"main line\x1b[?1049h");
        assert!(pane.in_alt_screen(), "alt screen active");
        assert_eq!(pane.grid().line_text(0).as_deref(), Some(""));

        pane.feed(b"alt line");
        assert_eq!(pane.grid().line_text(0).as_deref(), Some("alt line"));

        pane.feed(b"\x1b[?1049l");
        assert!(!pane.in_alt_screen(), "back on the main screen");
        assert_eq!(pane.grid().line_text(0).as_deref(), Some("main line"));
    }

    #[test]
    fn resize_reaches_the_shell() {
        let mut pane = Pane::new();
        assert!(matches!(pane.resize(100, 30), Err(ShellError::NotRunning)));
        assert_eq!((pane.grid().cols, pane.grid().rows), (100, 30));
    }

    #[test]
    fn resize_clears_the_selection() {
        let mut pane = Pane::new();
        pane.feed(b"line one\r\nline two\r\n");
        pane.set_selection(Some(Selection::between(
            Pos { row: 0, col: 0 },
            Pos { row: 1, col: 4 },
        )));
        assert!(pane.selection().is_some(), "selection set before resize");
        let _ = pane.resize(40, 12);
        assert!(
            pane.selection().is_none(),
            "reflow invalidates the selection's line ids"
        );
    }
}
