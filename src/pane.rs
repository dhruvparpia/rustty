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

use crate::graphics::GraphicsAction;
use crate::grid::Grid;
use crate::parser::{Parser, ParserEvent};
use crate::selection::Selection;
use crate::shell::{Shell, ShellError, ShellEvent, SpawnOptions};

/// Reply to a kitty capability query (`a=I`): meta code 0, data "OK".
const GRAPHICS_QUERY_OK: &[u8] = b"\x1b_G0;OK\x1b\\";
/// Reply to a kitty image query for an id that is not loaded (meta code 5).
const GRAPHICS_QUERY_MISS: &[u8] = b"\x1b_G5;IMGNOTFOUND\x1b\\";

/// Payload format of a stored kitty image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    /// PNG (`f=100`) or JPEG (`f=101`): the renderer decodes.
    Encoded,
    /// `f=24`: raw RGB, row-major; needs `w=`/`h=`.
    Rgb {
        /// Pixel width.
        width: u32,
        /// Pixel height.
        height: u32,
    },
    /// `f=32`: raw RGBA, row-major; needs `w=`/`h=`.
    Rgba {
        /// Pixel width.
        width: u32,
        /// Pixel height.
        height: u32,
    },
}

/// One decoded kitty image, keyed by its protocol id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredImage {
    /// Encoded (PNG/JPEG) or raw pixel bytes.
    pub data: Vec<u8>,
    /// Payload format.
    pub kind: ImageKind,
    /// Store generation at insert time: the renderer re-decodes on change.
    pub generation: u64,
}

/// Where one kitty image is shown: anchored to an absolute grid row, so it
/// scrolls with the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// Image id.
    pub id: u32,
    /// Absolute grid row of the image top.
    pub top_abs: usize,
    /// Grid column of the image left edge.
    pub col: usize,
    /// `c=`: columns to cover; 0 = native pixel width.
    pub cols: u16,
    /// `r=`: rows to cover; 0 = native pixel height.
    pub rows: u16,
    /// `t=1`: dropped once it scrolls out of view.
    pub transient: bool,
}

/// The pane's kitty images and placements (roadmap v0.5).
#[derive(Debug, Default)]
pub struct ImageStore {
    images: std::collections::BTreeMap<u32, StoredImage>,
    placements: Vec<Placement>,
    chunks: std::collections::BTreeMap<u32, Vec<u8>>,
    auto_id: u32,
    generation: u64,
}

impl ImageStore {
    /// Apply one decoded command at the cursor (`grid` resolves the
    /// absolute-row anchors). Returns reply bytes for queries.
    pub fn apply(
        &mut self,
        action: GraphicsAction,
        cursor_row: usize,
        cursor_col: usize,
        grid: &Grid,
    ) -> Option<Vec<u8>> {
        match action {
            GraphicsAction::Transmit {
                id,
                format,
                width,
                height,
                data,
                more,
            } => {
                let id = if id == 0 { self.next_id() } else { id };
                if more == 1 {
                    self.chunks.entry(id).or_default().extend_from_slice(&data);
                    return None;
                }
                let mut bytes = self.chunks.remove(&id).unwrap_or_default();
                bytes.extend_from_slice(&data);
                let kind = match format {
                    24 => ImageKind::Rgb { width, height },
                    32 => ImageKind::Rgba { width, height },
                    _ => ImageKind::Encoded,
                };
                self.generation += 1;
                self.images.insert(
                    id,
                    StoredImage {
                        data: bytes,
                        kind,
                        generation: self.generation,
                    },
                );
                // A completed transmission displays at the cursor unless
                // `m=2` (load only).
                if more != 2 {
                    self.placements.push(Placement {
                        id,
                        top_abs: Self::anchor(grid, cursor_row, 0),
                        col: cursor_col,
                        cols: 0,
                        rows: 0,
                        transient: false,
                    });
                }
                None
            }
            GraphicsAction::Place {
                id,
                x,
                y,
                cols,
                rows,
                transient,
            } => {
                if self.images.contains_key(&id) {
                    self.placements.push(Placement {
                        id,
                        top_abs: Self::anchor(grid, cursor_row, y),
                        col: i64::from(x)
                            .checked_add(cursor_col as i64)
                            .filter(|c| *c >= 0)
                            .map_or(0, |c| c as usize),
                        cols,
                        rows,
                        transient,
                    });
                }
                None
            }
            GraphicsAction::DeleteAll => {
                self.clear_all();
                None
            }
            GraphicsAction::DeleteId(id) => {
                self.images.remove(&id);
                self.placements.retain(|p| p.id != id);
                self.chunks.remove(&id);
                self.generation += 1;
                None
            }
            GraphicsAction::Query(None) => Some(GRAPHICS_QUERY_OK.to_vec()),
            GraphicsAction::Query(Some(id)) => Some(
                if self.images.contains_key(&id) {
                    GRAPHICS_QUERY_OK
                } else {
                    GRAPHICS_QUERY_MISS
                }
                .to_vec(),
            ),
        }
    }

    /// Drop every image and placement (screen clear, alternate-screen swap).
    pub fn clear_all(&mut self) {
        self.images.clear();
        self.placements.clear();
        self.chunks.clear();
        self.generation += 1;
    }

    /// Store generation: changes on every add, delete and clear. The
    /// renderer re-decodes changed images and repaints on generation jumps.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Drop transient placements that scrolled out of the viewport.
    pub fn prune_transient(&mut self, grid: &Grid) {
        self.placements
            .retain(|p| !p.transient || grid.view_of_abs(p.top_abs).is_some());
    }

    /// The stored image for `id`.
    #[must_use]
    pub fn image(&self, id: u32) -> Option<&StoredImage> {
        self.images.get(&id)
    }

    /// Every placement, in receive order.
    #[must_use]
    pub fn placements(&self) -> &[Placement] {
        &self.placements
    }

    /// Placements with the pane viewport row their top sits on.
    pub fn visible(&self, grid: &Grid) -> impl Iterator<Item = (&Placement, usize)> {
        self.placements
            .iter()
            .filter_map(move |p| grid.view_of_abs(p.top_abs).map(|row| (p, row)))
    }

    /// Cursor row `view_row` (+ row offset `y`) as an absolute grid row.
    fn anchor(grid: &Grid, view_row: usize, y: i32) -> usize {
        grid.abs_of_view(view_row)
            .unwrap_or_else(|| grid.last_abs() + 1)
            .saturating_add_signed(i64::from(y) as isize)
    }

    fn next_id(&mut self) -> u32 {
        self.auto_id = self.auto_id.wrapping_add(1).max(1);
        self.auto_id
    }
}

/// Turn an OSC 7 `file://host/path` (percent-encoded) into a path.
fn cwd_from_url(url: &str) -> String {
    let rest = url.strip_prefix("file://").unwrap_or(url);
    // Drop the host component (everything up to the first '/'), keeping a
    // leading slash for the absolute path that follows.
    let path = match rest.find('/') {
        Some(i) => &rest[i..],
        None => return String::new(),
    };
    // Issue #33: decode percent escapes to BYTES first, then decode
    // UTF-8. Decoding byte-per-char corrupted non-ASCII paths (`%C3%A9`
    // became "Ã©").
    let mut bytes = Vec::with_capacity(path.len());
    let mut chars = path.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2
                && let Ok(byte) = u8::from_str_radix(&hex, 16)
            {
                bytes.push(byte);
                continue;
            }
        }
        bytes.extend_from_slice(c.encode_utf8(&mut [0u8; 4]).as_bytes());
    }
    let out = String::from_utf8_lossy(&bytes).into_owned();
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
    /// BEL seen and not yet handled by the window layer (BELL-001).
    bell: bool,
    /// Main screen kept while the alternate screen is active (TTY-018).
    main_grid: Option<Grid>,
    in_alt: bool,
    /// Grid changed since the renderer last asked (repaint hint).
    dirty: bool,
    /// Mouse text selection, in absolute grid coordinates (WIN-004).
    selection: Option<Selection>,
    /// Kitty graphics images and placements (roadmap v0.5).
    graphics: ImageStore,
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
            bell: false,
            main_grid: None,
            in_alt: false,
            dirty: true,
            selection: None,
            graphics: ImageStore::default(),
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
            bell: false,
            main_grid: None,
            in_alt: false,
            dirty: true,
            selection: None,
            graphics: ImageStore::default(),
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

    /// The pane's kitty images and placements (roadmap v0.5), for the
    /// renderer's image draw pass.
    #[must_use]
    pub fn graphics(&self) -> &ImageStore {
        &self.graphics
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
    /// marker)? TAB-018, WINDOW-014 and the notification plan use this.
    #[must_use]
    pub fn prompt_ready(&self) -> bool {
        self.at_prompt
    }

    /// BELL-001: a bell is waiting to be shown or heard.
    #[must_use]
    pub fn bell_pending(&self) -> bool {
        self.bell
    }

    /// Clear the bell flag: the window layer rings once per BEL.
    pub fn take_bell(&mut self) {
        self.bell = false;
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
    ///
    /// Issue #1/#7: the parser stops after any event that changes which
    /// grid is drawn (alt-screen swap, scrollback clear); the loop applies
    /// the events and resumes parsing on the swapped grid, so one chunk
    /// containing `1049h` + text places the text on the screen the mode
    /// switch selects.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.dirty = true;
        let mut printed = false;
        let mut pos = 0usize;
        while pos < bytes.len() {
            let outcome = self.parser.advance(&mut self.grid, &bytes[pos..]);
            let step = outcome.consumed.min(bytes.len() - pos).max(1);
            pos += step;
            printed |= outcome.printed;

            for event in outcome.events {
                match event {
                    ParserEvent::Title(title) => self.title = Some(title),
                    ParserEvent::Cwd(url) => self.record_cwd(&url),
                    ParserEvent::Bell => self.bell = true,
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
                            let cols = self.grid.cols;
                            let rows = self.grid.rows;
                            let fresh = Grid::with_size(cols, rows, 0);
                            self.main_grid = Some(std::mem::replace(&mut self.grid, fresh));
                            self.in_alt = true;
                            self.graphics.clear_all();
                        }
                    }
                    ParserEvent::ExitAltScreen => {
                        if self.in_alt {
                            if let Some(main) = self.main_grid.take() {
                                self.grid = main;
                                let (cols, rows) = (self.grid.cols, self.grid.rows);
                                self.grid.resize(cols, rows);
                            }
                            self.in_alt = false;
                            self.graphics.clear_all();
                        }
                    }
                    // The parser owns the flag; the renderer reads it via
                    // `Pane::cursor_visible`, so there is nothing to apply.
                    ParserEvent::CursorVisibility(_) => {}
                    ParserEvent::ClearScrollback => {
                        // Issue #7: ED 3 / "clear history" removes the
                        // scrollback, never the visible screen.
                        self.grid.clear_scrollback();
                        self.graphics.clear_all();
                    }
                    // DSR/DA replies go straight back to the shell.
                    ParserEvent::Respond(reply) => {
                        let _ = self.shell.write_input(&reply);
                    }
                    ParserEvent::Graphics(action) => {
                        let (row, col) = (self.parser.cursor_row, self.parser.cursor_col);
                        if let Some(reply) = self.graphics.apply(action, row, col, &self.grid) {
                            let _ = self.shell.write_input(&reply);
                        }
                    }
                }
            }
        }

        self.graphics.prune_transient(&self.grid);

        if printed {
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
        // Issue #33: percent-escapes are UTF-8 bytes, not Latin-1 chars.
        pane.feed(b"\x1b]7;file://HOST/home/dhruv/caf%C3%A9\x07");
        assert_eq!(pane.cwd(), Some("/home/dhruv/café"));
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
    fn a_bell_is_pending_until_the_window_layer_takes_it() {
        let mut pane = Pane::with_size(20, 5, 10);
        pane.feed(b"make");
        assert!(pane.bell_pending());
        pane.take_bell();
        assert!(!pane.bell_pending());
        // An OSC terminator is not a bell.
        pane.feed(b"]0;T");
        assert!(!pane.bell_pending());
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

    #[test]
    fn kitty_transmit_stores_and_places_at_cursor() {
        let mut pane = Pane::with_size(10, 5, 20);
        pane.feed(b"\x1b[2;4H\x1b_Ga=t,f=24,w=1,h=1;AA==\x1b\\");
        let img = pane.graphics().image(1).expect("image stored");
        assert_eq!(img.data, vec![0]);
        assert_eq!(
            img.kind,
            ImageKind::Rgb {
                width: 1,
                height: 1
            }
        );
        let placements = pane.graphics().placements();
        assert_eq!(placements.len(), 1);
        assert_eq!((placements[0].top_abs, placements[0].col), (1, 3));
        assert_eq!(pane.graphics().visible(pane.grid()).count(), 1);
    }

    #[test]
    fn kitty_chunks_assemble_and_auto_ids_increment() {
        let mut pane = Pane::with_size(10, 5, 20);
        pane.feed(b"\x1b_Ga=t,m=1,i=9;YQ\x1b\\");
        assert!(pane.graphics().image(9).is_none(), "m=1 only buffers");
        pane.feed(b"\x1b_Ga=t,m=0,i=9;Yg==\x1b\\");
        assert_eq!(pane.graphics().image(9).expect("assembled").data, b"ab");
        pane.feed(b"\x1b_Ga=t,m=2;AA==\x1b\\");
        assert!(pane.graphics().image(1).is_some(), "auto id");
        assert_eq!(
            pane.graphics().placements().len(),
            1,
            "m=2 loads, no placement"
        );
    }

    #[test]
    fn kitty_delete_id_drops_image_and_placements() {
        let mut pane = Pane::with_size(10, 5, 20);
        pane.feed(b"\x1b_Ga=t,i=3,f=24,w=1,h=1;AA==\x1b\\");
        pane.feed(b"\x1b_Ga=d,i=3\x1b\\");
        assert!(pane.graphics().image(3).is_none());
        assert!(pane.graphics().placements().is_empty());
    }

    #[test]
    fn kitty_alt_screen_clears_images() {
        let mut pane = Pane::with_size(10, 5, 20);
        pane.feed(b"\x1b_Ga=t,f=24,w=1,h=1;AA==\x1b\\");
        pane.feed(b"\x1b[?1049h");
        assert!(pane.graphics().image(1).is_none());
        pane.feed(b"\x1b[?1049l");
        assert!(
            pane.graphics().image(1).is_none(),
            "main screen images are gone too"
        );
    }

    #[test]
    fn kitty_transient_placements_prune_on_scroll() {
        let mut pane = Pane::with_size(10, 5, 20);
        pane.feed(b"\x1b_Ga=t,f=24,w=1,h=1,m=2,i=1;AA==\x1b\\");
        pane.feed(b"\x1b_Ga=p,i=1,t=1\x1b\\\x1b_Ga=p,i=1\x1b\\");
        assert_eq!(pane.graphics().placements().len(), 2);
        pane.feed(b"\r\na\r\nb\r\nc\r\nd\r\ne\r\nf");
        assert_eq!(pane.graphics().placements().len(), 1, "transient pruned");
        assert!(!pane.graphics().placements()[0].transient);
    }

    #[test]
    fn kitty_query_replies() {
        let mut store = ImageStore::default();
        let grid = Grid::with_size(10, 5, 0);
        assert_eq!(
            store.apply(GraphicsAction::Query(None), 0, 0, &grid),
            Some(GRAPHICS_QUERY_OK.to_vec())
        );
        assert_eq!(
            store.apply(GraphicsAction::Query(Some(1)), 0, 0, &grid),
            Some(GRAPHICS_QUERY_MISS.to_vec())
        );
        store.apply(
            GraphicsAction::Transmit {
                id: 1,
                format: 100,
                width: 0,
                height: 0,
                data: vec![1],
                more: 0,
            },
            0,
            0,
            &grid,
        );
        assert_eq!(
            store.apply(GraphicsAction::Query(Some(1)), 0, 0, &grid),
            Some(GRAPHICS_QUERY_OK.to_vec())
        );
    }
}
