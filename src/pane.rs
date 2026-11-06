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

use crate::command::CommandTracker;
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

/// Payload bytes plus the header the transmission started with. Issue
/// #32: continuation frames carry only `m=1`, so format and dimensions
/// must survive from the first frame until `m=0` finalizes the image.
#[derive(Debug)]
struct Chunk {
    data: Vec<u8>,
    format: u32,
    width: u32,
    height: u32,
    display: bool,
}

/// The pane's kitty images and placements (roadmap v0.5).
#[derive(Debug)]
pub struct ImageStore {
    images: std::collections::BTreeMap<u32, StoredImage>,
    placements: Vec<Placement>,
    chunks: std::collections::BTreeMap<u32, Chunk>,
    auto_id: u32,
    generation: u64,
    /// Process-unique id (issue #35): the renderer's bitmap cache is keyed
    /// by (store, image id), so two tabs' panes never share a key.
    store_id: u64,
}

impl Default for ImageStore {
    fn default() -> Self {
        static NEXT_STORE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            images: std::collections::BTreeMap::new(),
            placements: Vec::new(),
            chunks: std::collections::BTreeMap::new(),
            auto_id: 0,
            generation: 0,
            store_id: NEXT_STORE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }
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
                display,
            } => {
                // Auto ids: the first frame of a group assigns one; later
                // frames (id=0) join the open group (issue #32).
                // shortcut: the protocol assembles one group at a time; a
                // single-frame id=0 command sent mid-chunk joins that chunk.
                let id = if id != 0 {
                    id
                } else if let Some((&open, _)) = self.chunks.iter().next_back() {
                    open
                } else {
                    self.next_id()
                };
                if more == 1 {
                    let chunk = self.chunks.entry(id).or_insert(Chunk {
                        data: Vec::new(),
                        format,
                        width,
                        height,
                        display,
                    });
                    chunk.data.extend_from_slice(&data);
                    return None;
                }
                // `m=0` completes the transmission; metadata comes from the
                // first frame (issue #32).
                let (mut bytes, format, width, height, display) = match self.chunks.remove(&id) {
                    Some(c) => (c.data, c.format, c.width, c.height, c.display),
                    None => (Vec::new(), format, width, height, display),
                };
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
                // Only `a=T` displays at the cursor; `a=t` loads and waits
                // for an explicit `a=p` (issue #31).
                if display {
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

    /// Process-unique store identity for the renderer's bitmap cache
    /// (issue #35).
    #[must_use]
    pub const fn store_id(&self) -> u64 {
        self.store_id
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

    /// Live cursor row `cursor_row` (+ row offset `y`) as an absolute grid
    /// row, independent of where the viewport is scrolled.
    fn anchor(grid: &Grid, cursor_row: usize, y: i32) -> usize {
        grid.live_abs(cursor_row)
            .unwrap_or_else(|| grid.last_abs() + 1)
            .saturating_add_signed(i64::from(y) as isize)
    }

    fn next_id(&mut self) -> u32 {
        self.auto_id = self.auto_id.wrapping_add(1).max(1);
        self.auto_id
    }
}

/// Host component of an OSC 7 `file://host/path` URL ("" when the URL has
/// no host, `file:///path`).
fn host_from_url(url: &str) -> &str {
    let rest = url.strip_prefix("file://").unwrap_or(url);
    match rest.find('/') {
        Some(i) => &rest[..i],
        None => rest,
    }
}

/// Issue #40 (IDENT-001): is an OSC 7 host this machine? "localhost", an
/// empty host, and the local computer name all mean local. No prompt-text
/// guessing: an unknown session simply reports nothing.
fn host_is_local(host: &str) -> bool {
    host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || std::env::var("COMPUTERNAME")
            .is_ok_and(|name| !name.is_empty() && name.eq_ignore_ascii_case(host))
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
    /// Stable identity (AUTO-001): assigned at creation, never reused.
    id: u64,
    grid: Grid,
    shell: Shell,
    parser: Parser,
    title: Option<String>,
    /// Last working directory the shell reported (OSC 7, SHELL-001).
    cwd: Option<String>,
    /// OSC 7 host that is not this machine: the session is remote
    /// (IDENT-001). `None` means local or unknown; nothing is guessed.
    shell_host: Option<String>,
    /// OSC 133;A/133;D seen: the shell is at a prompt.
    at_prompt: bool,
    /// OSC 133;B/C seen without a matching `D`: a command is running.
    command_running: bool,
    /// Exit code from the last `OSC 133;D:<code>`.
    last_exit: Option<i32>,
    /// Per-command records built from OSC 133 transitions (SHELLINT-006/
    /// 010/013-019, issue #38). Empty for shells that emit no OSC 133.
    commands: CommandTracker,
    /// BEL seen and not yet handled by the window layer (BELL-001).
    bell: bool,
    /// Main screen kept while the alternate screen is active (TTY-018).
    main_grid: Option<Grid>,
    in_alt: bool,
    /// The user has sent input (see [`Pane::input_seen`]).
    input_seen: bool,
    /// A bell or notification nobody has seen yet.
    attention: bool,
    /// What the last notification said.
    attention_message: Option<String>,
    /// A program is reporting progress.
    progress: bool,
    /// When output last arrived.
    last_output: Option<std::time::Instant>,
    /// When the user last sent input.
    last_input: Option<std::time::Instant>,
    /// When the grid last changed size.
    last_resize: Option<std::time::Instant>,
    /// The agent CLI running here now (RESTORE-002).
    agent: Option<&'static str>,
    /// The agent a restart stopped here, until it is resumed or dismissed.
    interrupted: Option<String>,
    /// Where the agent, or else the shell, is really running.
    process_cwd: Option<String>,
    /// What programs have reported about themselves (STATUS-001).
    program_status: crate::program_status::Records,
    /// What the agent is asking on screen (STATUS-002).
    screen_question: Option<String>,
    /// The output the screen was last read at.
    screen_read_at: Option<std::time::Instant>,
    /// What the sessions sidebar remembers about this pane.
    activity: crate::rail::Activity,
    /// The status from the last sidebar step, for readers that must not step.
    rail_status: crate::rail::Status,
    /// When automation last typed here (AUTO-011: shown on the sidebar row).
    automation_input: Option<std::time::Instant>,
    /// Grid changed since the renderer last asked (repaint hint).
    dirty: bool,
    /// Mouse text selection, in absolute grid coordinates (WIN-004).
    selection: Option<Selection>,
    /// Bookmarks over this pane's history (issue #53, BKMK-001..003),
    /// sorted by anchor.
    bookmarks: Vec<crate::bookmark::Bookmark>,
    /// Kitty graphics images and placements (roadmap v0.5).
    graphics: ImageStore,
    /// Viewport follows new output at the live edge (issue #42). Scrolling
    /// away pauses it so reading scrollback survives a burst of output;
    /// returning to the live edge resumes it.
    follow: bool,
    /// Host alias when this pane is a session view (issue #49).
    view_alias: Option<u32>,
    /// Whether this pane's view holds controller rights (issue #49).
    view_controller: bool,
}

impl Pane {
    /// Create a pane with a fresh grid, a fresh parser and an inactive shell.
    #[must_use]
    pub fn new() -> Self {
        Self {
            grid: Grid::new(),
            id: crate::automation::next_id(),
            shell: Shell::new(),
            parser: Parser::new(),
            title: None,
            cwd: None,
            shell_host: None,
            at_prompt: false,
            command_running: false,
            last_exit: None,
            commands: CommandTracker::new(),
            bell: false,
            main_grid: None,
            in_alt: false,
            input_seen: false,
            attention: false,
            attention_message: None,
            progress: false,
            last_output: None,
            last_input: None,
            last_resize: None,
            agent: None,
            interrupted: None,
            process_cwd: None,
            program_status: crate::program_status::Records::default(),
            screen_question: None,
            screen_read_at: None,
            activity: crate::rail::Activity::default(),
            rail_status: crate::rail::Status::Idle,
            automation_input: None,
            dirty: true,
            selection: None,
            bookmarks: Vec::new(),
            graphics: ImageStore::default(),
            follow: true,
            view_alias: None,
            view_controller: false,
        }
    }

    /// Create a pane whose grid starts at `cols` x `rows` with a
    /// `scrollback`-line history (config `scrollback`).
    #[must_use]
    pub fn with_size(cols: usize, rows: usize, scrollback: usize) -> Self {
        Self {
            grid: Grid::with_size(cols, rows, scrollback),
            id: crate::automation::next_id(),
            shell: Shell::new(),
            parser: Parser::new(),
            title: None,
            cwd: None,
            shell_host: None,
            at_prompt: false,
            command_running: false,
            last_exit: None,
            commands: CommandTracker::new(),
            bell: false,
            main_grid: None,
            in_alt: false,
            input_seen: false,
            attention: false,
            attention_message: None,
            progress: false,
            last_output: None,
            last_input: None,
            last_resize: None,
            agent: None,
            interrupted: None,
            process_cwd: None,
            program_status: crate::program_status::Records::default(),
            screen_question: None,
            screen_read_at: None,
            activity: crate::rail::Activity::default(),
            rail_status: crate::rail::Status::Idle,
            automation_input: None,
            dirty: true,
            selection: None,
            bookmarks: Vec::new(),
            graphics: ImageStore::default(),
            follow: true,
            view_alias: None,
            view_controller: false,
        }
    }

    /// Stable identity (AUTO-001): assigned at creation, never reused.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// The grid holding this pane's history: the main screen, even while the
    /// alternate screen is showing. The alternate grid numbers its lines
    /// independently, so a scrollback cursor only means something here.
    #[must_use]
    pub fn history_grid(&self) -> &Grid {
        self.main_grid.as_ref().unwrap_or(&self.grid)
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

    /// This pane's bookmarks, sorted by anchor (issue #53, BKMK-001).
    #[must_use]
    pub fn bookmarks(&self) -> &[crate::bookmark::Bookmark] {
        &self.bookmarks
    }

    /// This pane's bookmarks, mutably (toggle/delete in the window layer).
    pub fn bookmarks_mut(&mut self) -> &mut Vec<crate::bookmark::Bookmark> {
        &mut self.bookmarks
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

    /// OSC 7 host that is not this machine (IDENT-001): `Some` means the
    /// session is remote. `None` means local or unknown.
    #[must_use]
    pub fn shell_host(&self) -> Option<&str> {
        self.shell_host.as_deref()
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

    /// Bounded per-command history built from OSC 133 transitions
    /// (SHELLINT-006/010/013-019, issue #38). Empty for shells that emit
    /// no OSC 133.
    #[must_use]
    pub fn commands(&self) -> &CommandTracker {
        &self.commands
    }

    /// Text of command `id`'s output block: grid lines from its first
    /// output line to its end (SHELLINT-023). `None` when the command is
    /// unknown, still running, or its output has fully scrolled away.
    #[must_use]
    pub fn command_output_text(&self, id: u64) -> Option<String> {
        let cmd = self.commands.iter().find(|c| c.id == id)?;
        let end = cmd.end_abs?;
        let mut out = String::new();
        for abs in cmd.output_start_abs..=end {
            if let Some(text) = self.grid.line_text_abs(abs) {
                out.push_str(&text);
                out.push('\n');
            }
        }
        while out.ends_with('\n') {
            out.pop();
        }
        (!out.is_empty()).then_some(out)
    }

    /// Turn a `file://host/path` report into a path string.
    fn record_cwd(&mut self, url: &str) {
        self.cwd = Some(cwd_from_url(url));
        let host = host_from_url(url);
        self.shell_host = (!host_is_local(host)).then(|| host.to_string());
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

    /// Absolute grid line under the cursor: where the next printed byte
    /// lands, so OSC 133 marks record command positions relative to it
    /// (the grid preallocates every screen row, so `last_abs` is the
    /// bottom row, not the cursor). Also the bookmark anchor when the
    /// toggle is not driven by a search match (BKMK-001, issue #53).
    #[must_use]
    pub fn cursor_abs(&self) -> usize {
        self.grid
            .live_abs(self.parser.cursor_row)
            .unwrap_or_else(|| self.grid.last_abs())
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
        if crate::rail::output_is_work(self.last_resize.map(|at| at.elapsed())) {
            self.last_output = Some(std::time::Instant::now());
        }
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
                    ParserEvent::CwdPath(path) => {
                        let path = path.trim_matches('"').replace('\\', "/");
                        if !path.is_empty() {
                            self.cwd = Some(path);
                            self.shell_host = None;
                        }
                    }
                    // The parser owns the link pen and the URI table; the
                    // renderer and window layer read them on demand.
                    ParserEvent::Hyperlink(_) => {}
                    ParserEvent::Bell => {
                        self.bell = true;
                        self.attention = true;
                    }
                    ParserEvent::Notify(text) => {
                        self.attention = true;
                        self.attention_message = Some(crate::rail::clean_message(&text));
                    }
                    ParserEvent::Progress(active) => self.progress = active,
                    ParserEvent::ProgramStatus(report) => self.program_status.apply(report),
                    ParserEvent::PromptStart => {
                        self.progress = false;
                        self.program_status.program_gone();
                        self.at_prompt = true;
                        self.command_running = false;
                        let abs = self.cursor_abs();
                        self.commands.prompt_start(abs, CommandTracker::now_ms());
                    }
                    ParserEvent::CommandStart => {
                        self.command_running = true;
                        self.at_prompt = false;
                        let abs = self.cursor_abs();
                        self.commands.command_start(
                            abs,
                            self.cwd.clone(),
                            CommandTracker::now_ms(),
                        );
                    }
                    ParserEvent::CommandOutput => {
                        self.command_running = true;
                        self.at_prompt = false;
                        let abs = self.cursor_abs();
                        self.commands.command_output(abs);
                    }
                    ParserEvent::PromptEnd(code) => {
                        self.at_prompt = true;
                        self.command_running = false;
                        self.last_exit = code;
                        let abs = self.cursor_abs();
                        self.commands
                            .prompt_end(abs, code, CommandTracker::now_ms());
                    }
                    ParserEvent::EnterAltScreen => {
                        if !self.in_alt {
                            // The running command's output belongs to the alt
                            // app, not to a scrollback block (SHELLINT-026).
                            let abs = self.cursor_abs();
                            self.commands
                                .interrupt_running(abs, CommandTracker::now_ms());
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
                        // Commands whose block lived in the cleared history
                        // are no longer showable (SHELLINT-021).
                        self.retire_evicted_commands();
                    }
                    // DSR/DA replies go back to the shell only for a local
                    // shell. A host-owned session already answers terminal
                    // queries, so a controller view must not add a second reply
                    // (issue #70).
                    ParserEvent::Respond(reply) => {
                        if self.view_alias.is_none() {
                            let _ = self.shell.write_input(&reply);
                        }
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

        // Scrollback lines evicted during this feed retire their commands.
        self.retire_evicted_commands();

        if printed && self.follow {
            let above = self.grid.scroll_offset();
            if above > 0 {
                self.grid.scroll_down(above);
            }
        }
        self.parser.fit(&self.grid);
    }

    /// Drop commands whose block left the main grid. Command anchors are
    /// main-grid line ids; the alt grid numbers its lines independently, so
    /// its evictions say nothing about them.
    fn retire_evicted_commands(&mut self) {
        if !self.in_alt {
            self.commands.note_eviction(self.grid.first_abs());
        }
    }

    /// Size a pane nothing has been written to yet, so text can go in at the
    /// window's real size ahead of the shell. It rebuilds the grid: resizing
    /// would push the empty top line into scrollback when shrinking, and a
    /// later grow would bring it back above that text.
    pub fn fit_grid(&mut self, cols: usize, rows: usize) {
        self.grid = Grid::with_size(cols, rows, self.grid.scrollback());
        self.parser.fit(&self.grid);
        self.dirty = true;
    }

    /// Colors for ANSI-16 output from now on. Text already on screen keeps
    /// the colors it was written with.
    pub const fn set_palette(&mut self, palette: [crate::color::Color; 16]) {
        self.parser.palette = palette;
    }

    /// Whether the viewport follows new output at the live edge (issue #42).
    #[must_use]
    pub const fn follow(&self) -> bool {
        self.follow
    }

    /// Re-derive follow from the viewport: at the live edge (scroll offset
    /// 0) new output snaps the view; scrolled away, the reading position
    /// holds. Call after every user scroll (issue #42).
    pub fn update_follow(&mut self) {
        self.follow = self.grid.scroll_offset() == 0;
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
                ShellEvent::Resize(cols, rows) => {
                    // A view's geometry is canonical from the host
                    // (issue #49): fit the grids, never push back.
                    self.selection = None;
                    self.grid.resize(usize::from(cols), usize::from(rows));
                    if let Some(main) = &mut self.main_grid {
                        main.resize(usize::from(cols), usize::from(rows));
                    }
                    self.parser.fit(&self.grid);
                    self.dirty = true;
                }
                ShellEvent::ControlChanged(c) => {
                    self.view_controller = c;
                }
                ShellEvent::Notice(msg) => {
                    self.feed(format!("\r\n[{msg}]\r\n").as_bytes());
                }
            }
        }
    }

    /// Attach this pane as a view of live host session `alias`
    /// (issue #49). Closing the view detaches; the session keeps running.
    ///
    /// # Errors
    /// No host, attach refusals, thread setup failures.
    pub fn attach_view(&mut self, alias: u32, take_control: bool) -> Result<(), String> {
        self.shell.attach_view(alias, take_control)?;
        self.view_alias = Some(alias);
        self.view_controller = self.shell.view_controller();
        Ok(())
    }

    /// Host alias when this pane is a session view (issue #49).
    #[must_use]
    pub const fn view_alias(&self) -> Option<u32> {
        self.view_alias
    }

    /// Whether this pane's view holds controller rights (issue #49).
    #[must_use]
    pub const fn view_controller(&self) -> bool {
        self.view_controller
    }

    /// Ask this pane's view to take controller rights (issue #49).
    pub fn take_control(&self) {
        self.shell.take_control();
    }

    /// Start the pane's shell process (TTY-001, TTY-002).
    pub fn spawn_shell(&mut self, opts: &SpawnOptions) -> Result<(), ShellError> {
        self.shell.spawn(opts)
    }

    /// Send keyboard/paste bytes to the shell (TTY-004..TTY-017).
    pub fn send_input(&mut self, bytes: &[u8]) -> Result<(), ShellError> {
        self.input_seen = true;
        self.interrupted = None;
        self.last_input = Some(std::time::Instant::now());
        self.shell.write_input(bytes)
    }

    /// Whether a bell or notification is waiting to be seen.
    #[must_use]
    pub const fn needs_attention(&self) -> bool {
        self.attention
    }

    /// What the last unseen notification said, if it said anything.
    #[must_use]
    pub fn attention_message(&self) -> Option<&str> {
        if self.attention {
            self.attention_message.as_deref()
        } else {
            None
        }
    }

    /// The person has seen this pane.
    pub fn mark_seen(&mut self) {
        self.attention = false;
        self.attention_message = None;
    }

    /// Whether a program is reporting progress.
    #[must_use]
    pub const fn progress_active(&self) -> bool {
        self.progress
    }

    /// Time since the last output.
    #[must_use]
    pub fn output_age(&self) -> Option<std::time::Duration> {
        self.last_output.map(|at| at.elapsed())
    }

    /// Time since the last input.
    #[must_use]
    pub fn input_age(&self) -> Option<std::time::Duration> {
        self.last_input.map(|at| at.elapsed())
    }

    /// Record the agent CLI now running here.
    pub fn set_agent(&mut self, agent: Option<&'static str>) {
        if self.agent.is_some() && agent.is_none() {
            // It ended without a new prompt to say so.
            self.program_status.program_gone();
        }
        self.agent = agent;
        if agent.is_some() {
            self.interrupted = None;
        }
    }

    /// Record that a restart stopped `agent` here.
    pub fn set_interrupted(&mut self, agent: Option<String>) {
        self.interrupted = agent;
    }

    /// The agent a restart stopped here.
    #[must_use]
    pub fn interrupted(&self) -> Option<&str> {
        self.interrupted.as_deref()
    }

    /// Record where the agent, or else the shell, is running.
    pub fn set_process_cwd(&mut self, cwd: Option<String>) {
        self.process_cwd = cwd;
    }

    /// The folder this pane is really in. A running agent's own folder wins.
    /// Next comes what the shell reported, since `cd` in PowerShell does not
    /// move its process.
    #[must_use]
    pub fn saved_cwd(&self) -> Option<&str> {
        if self.agent.is_some() && self.process_cwd.is_some() {
            return self.process_cwd.as_deref();
        }
        self.cwd.as_deref().or(self.process_cwd.as_deref())
    }

    /// The agent to remember across a restart. A resume runs in the saved
    /// folder, so without one there is nothing safe to offer.
    #[must_use]
    pub fn saved_agent(&self) -> Option<&str> {
        self.interrupted
            .as_deref()
            .or_else(|| self.agent.filter(|_| self.saved_cwd().is_some()))
    }

    /// Read the agent's question off the screen, when new output arrived.
    fn read_screen(&mut self) {
        /// Rows from the bottom a prompt can occupy.
        const ROWS: usize = 24;
        let Some(agent) = self.agent else {
            self.screen_question = None;
            return;
        };
        if self.screen_read_at == self.last_output {
            return;
        }
        self.screen_read_at = self.last_output;
        let rows = self.grid.rows;
        let lines: Vec<String> = (rows.saturating_sub(ROWS)..rows)
            .filter_map(|row| self.grid.line_text(row))
            .collect();
        self.screen_question = crate::agents::waiting_on_screen(agent, &lines);
    }

    /// The status a program reported, when it reported one.
    fn reported_status(&mut self, viewed: bool) -> Option<crate::rail::Status> {
        use crate::program_status::State;
        use crate::rail::Status;
        let state = self.program_status.summary()?.state;
        Some(match state {
            State::Blocked => Status::NeedsYou,
            State::Working => Status::Working,
            State::Idle => Status::Idle,
            State::Done | State::Error => {
                if viewed {
                    self.program_status.mark_seen();
                    return self.reported_status(viewed);
                }
                if state == State::Done {
                    Status::Finished
                } else {
                    Status::Failed
                }
            }
        })
    }

    /// What the row should say for `status`, when there is more to say than
    /// its name: a program's report, or an unseen notification.
    #[must_use]
    pub fn status_message(&self, status: crate::rail::Status) -> Option<String> {
        use crate::rail::Status;
        if let Some(record) = self.program_status.summary() {
            if let Some(msg) = &record.msg {
                return Some(crate::rail::clean_message(msg));
            }
            if status == Status::NeedsYou {
                let said = match record.kind.as_deref() {
                    Some("permission") => "Needs your permission",
                    Some("question") => "Has a question for you",
                    Some("auth") => "Needs you to sign in",
                    _ => return None,
                };
                return Some(said.to_string());
            }
            return None;
        }
        if status != Status::NeedsYou {
            return None;
        }
        self.screen_question
            .clone()
            .or_else(|| self.attention_message().map(str::to_string))
    }

    /// Forget work the sidebar was not watching, so showing it again does not
    /// report a false "Finished".
    pub const fn reset_rail_activity(&mut self) {
        self.activity.reset();
    }

    /// This pane's status for the sessions sidebar. `viewed` means the person
    /// can see it, which also clears anything waiting to be seen.
    pub fn rail_step(&mut self, viewed: bool, now_ms: u64) -> crate::rail::Status {
        let status = self.step_activity(viewed, now_ms);
        self.rail_status = status;
        status
    }

    /// The status from the last [`Pane::rail_step`]. Stepping mutates what the
    /// sidebar remembers, so anything else that wants the status reads this.
    #[must_use]
    pub const fn rail_status(&self) -> crate::rail::Status {
        self.rail_status
    }

    /// The line the sidebar shows for the last computed status.
    #[must_use]
    pub fn rail_detail(&self) -> String {
        crate::rail::tab_detail(
            self.rail_status,
            self.interrupted(),
            self.attention_message(),
        )
    }

    /// Record that automation just typed into this pane (AUTO-011).
    pub fn note_automation_input(&mut self) {
        self.automation_input = Some(std::time::Instant::now());
    }

    /// Time since automation last typed into this pane.
    #[must_use]
    pub fn automation_input_age(&self) -> Option<std::time::Duration> {
        self.automation_input.map(|at| at.elapsed())
    }

    fn step_activity(&mut self, viewed: bool, now_ms: u64) -> crate::rail::Status {
        if viewed {
            self.mark_seen();
        }
        self.read_screen();
        // Claude Code's title says when it is busy, and its status bar redraws
        // when it is not, so its output says nothing.
        let title_tells = self.agent == Some("claude");
        let signals = crate::rail::Signals {
            title: self.title.as_deref().unwrap_or(""),
            command_running: self.command_running,
            exited: matches!(self.shell.state(), crate::shell::ShellState::Exited(_)),
            attention: self.attention,
            progress: self.progress,
            output_age: self.output_age().filter(|_| !title_tells),
            input_age: self.input_age(),
        };
        let status = self.activity.step(&signals, viewed, now_ms);
        let exited = signals.exited;
        if exited {
            self.program_status.program_gone();
        }
        // A program saying what it is doing beats any guess from the outside.
        if let Some(reported) = self.reported_status(viewed) {
            return reported;
        }
        if self.screen_question.is_some() && !exited {
            return crate::rail::Status::NeedsYou;
        }
        if self.interrupted.is_some() && !exited && status != crate::rail::Status::NeedsYou {
            return crate::rail::Status::Interrupted;
        }
        status
    }

    /// Whether the user has sent this pane any input. Terminal replies do not
    /// count.
    #[must_use]
    pub const fn input_seen(&self) -> bool {
        self.input_seen
    }

    /// Whether the pane is showing the alternate screen.
    #[must_use]
    pub const fn in_alt(&self) -> bool {
        self.in_alt
    }

    /// Ask for a repaint after writing to the grid directly.
    pub const fn mark_dirty(&mut self) {
        self.dirty = true;
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
        self.last_resize = Some(std::time::Instant::now());
        self.selection = None;
        if self.view_alias.is_some() {
            // A view's geometry is canonical from the host (issue
            // #49): the controller pushes its size, a watcher's grid
            // follows the host's Resize events instead.
            let _ = self.shell.resize(cols, rows);
            return Ok(());
        }
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

    #[test]
    fn panes_get_distinct_ids_that_only_go_up() {
        // Break caught: a closed pane's id naming a new pane.
        let a = Pane::new();
        let b = Pane::with_size(10, 4, 10);
        assert_ne!(a.id(), 0);
        assert!(b.id() > a.id());
    }

    #[test]
    fn a_pane_remembers_the_status_it_last_stepped_to() {
        // Break caught: automation stepping activity a second time per tick,
        // which would clear "finished" and attention the sidebar still shows.
        let mut pane = Pane::new();
        assert_eq!(pane.rail_status(), crate::rail::Status::Idle);
        pane.feed(b"\x1b]9;4;3;\x07");
        let stepped = pane.rail_step(false, 0);
        assert_eq!(stepped, crate::rail::Status::Working);
        assert_eq!(pane.rail_status(), stepped);
        assert_eq!(pane.rail_status(), stepped, "reading does not step");
    }

    #[test]
    fn history_stays_the_main_screen_while_the_alternate_one_shows() {
        // Break caught: a scrollback cursor reading the alt grid, whose line
        // numbers are unrelated, and returning the wrong lines.
        let mut pane = Pane::with_size(20, 3, 50);
        pane.feed(b"main text\r\n");
        let main_first = pane.history_grid().line_text_abs(0);
        pane.feed(b"\x1b[?1049halt text");
        assert!(pane.in_alt());
        assert_eq!(pane.history_grid().line_text_abs(0), main_first);
        assert_eq!(pane.grid().line_text_abs(0).as_deref(), Some("alt text"));
    }

    #[test]
    fn automation_input_is_recorded_for_the_sidebar() {
        let mut pane = Pane::new();
        assert_eq!(pane.automation_input_age(), None);
        pane.note_automation_input();
        assert!(pane.automation_input_age().is_some());
    }
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
    fn osc_7_host_is_the_session_identity() {
        // IDENT-001 (issue #40): remote hosts surface, local stays quiet.
        let mut pane = Pane::with_size(20, 5, 10);
        pane.feed(b"\x1b]7;file://db1.example/home/me\x07");
        assert_eq!(pane.shell_host(), Some("db1.example"));
        pane.feed(b"\x1b]7;file://localhost/C:/src\x07");
        assert_eq!(pane.shell_host(), None);
        pane.feed(b"\x1b]7;file:///home/me\x07");
        assert_eq!(pane.shell_host(), None);
        // The local computer name is local, case-insensitively.
        if let Ok(name) = std::env::var("COMPUTERNAME") {
            pane.feed(format!("\x1b]7;file://{name}/C:/src\x07").as_bytes());
            assert_eq!(pane.shell_host(), None);
        }
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
    fn osc_133_feed_records_a_command_with_its_output_block() {
        let mut pane = Pane::with_size(20, 5, 100);
        // Real shells emit the OSC marks in their own PTY chunks, so the
        // stamps land on the grid position at the time of the mark.
        pane.feed(b"prompt\r\nls\r\n");
        pane.feed(b"\x1b]133;B\x07");
        pane.feed(b"\x1b]133;C\x07");
        pane.feed(b"a.txt\r\nb.txt\r\n");
        pane.feed(b"\x1b]133;D:0\x07");
        let cmds: Vec<_> = pane.commands().iter().collect();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].exit, Some(0));
        assert!(!cmds[0].running);
        let text = pane.command_output_text(cmds[0].id).expect("output text");
        assert_eq!(text, "a.txt\nb.txt");
    }

    #[test]
    fn alt_screen_entry_interrupts_the_running_command() {
        let mut pane = Pane::with_size(20, 5, 100);
        pane.feed(b"\x1b]133;B\x07vim\r\n");
        assert!(pane.commands().running().is_some());
        pane.feed(b"\x1b[?1049h");
        assert!(pane.commands().running().is_none());
        assert_eq!(pane.commands().last_finished().and_then(|c| c.exit), None);
    }

    #[test]
    fn command_boundaries_drive_prev_and_next_navigation() {
        let mut pane = Pane::with_size(20, 5, 100);
        for i in 0..3u8 {
            pane.feed(format!("prompt {i}\r\n").as_bytes());
            pane.feed(b"\x1b]133;A\x07");
            pane.feed(b"\x1b]133;B\x07");
            pane.feed(b"out\r\n");
            pane.feed(b"\x1b]133;D:0\x07");
        }
        assert_eq!(pane.commands().len(), 3);
        let first = pane.commands().iter().next().expect("three commands");
        assert_eq!(pane.commands().next_boundary(first.start_abs), Some(3));
        let last_abs = pane.grid().last_abs();
        assert_eq!(pane.commands().prev_boundary(last_abs), Some(5));
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
        pane.feed(b"\x1b[2;4H\x1b_Ga=T,t=d,f=24,s=1,v=1;AA==\x1b\\");
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
    fn kitty_load_only_waits_for_explicit_place() {
        // Issue #31: `a=t` stores with no placement; `a=p` shows it.
        let mut pane = Pane::with_size(10, 5, 20);
        pane.feed(b"\x1b_Ga=t,f=24,w=1,h=1,i=5;AA==\x1b\\");
        assert!(pane.graphics().image(5).is_some(), "a=t stores the image");
        assert!(
            pane.graphics().placements().is_empty(),
            "a=t must not display"
        );
        pane.feed(b"\x1b_Ga=p,i=5\x1b\\");
        assert_eq!(pane.graphics().placements().len(), 1, "a=p displays it");
    }

    #[test]
    fn kitty_chunks_carry_metadata_and_assemble() {
        // Issue #32: continuation frames carry only `m=`; format and
        // dimensions come from the first frame, placement from `a=T`.
        // Issue #30: `s=`/`v=` size the raw payload.
        let mut pane = Pane::with_size(10, 5, 20);
        pane.feed(b"\x1b_Ga=T,t=d,f=24,s=2,v=1,i=9,m=1;AAA=\x1b\\");
        assert!(pane.graphics().image(9).is_none(), "m=1 only buffers");
        pane.feed(b"\x1b_Gm=1;AAA=\x1b\\"); // continuation: only m, per the spec
        pane.feed(b"\x1b_Gm=0;AAA=\x1b\\");
        let img = pane.graphics().image(9).expect("assembled");
        assert_eq!(img.data, vec![0; 6], "2x1 RGB");
        assert_eq!(
            img.kind,
            ImageKind::Rgb {
                width: 2,
                height: 1
            },
            "the final frame carries no f/s/v; the first frame's do"
        );
        assert_eq!(pane.graphics().placements().len(), 1, "a=T displays");
    }

    #[test]
    fn kitty_auto_ids_increment_and_chunks_join_the_group() {
        let mut pane = Pane::with_size(10, 5, 20);
        pane.feed(b"\x1b_Ga=t,m=1;YQ==\x1b\\"); // id=0 first frame assigns 1
        pane.feed(b"\x1b_Gm=0;Yg==\x1b\\"); // id=0 completion joins group 1
        assert_eq!(
            pane.graphics().image(1).expect("assembled").data,
            b"ab",
            "the id=0 continuation joined the id=0 group"
        );
        pane.feed(b"\x1b_Ga=t;AA==\x1b\\");
        assert!(pane.graphics().image(2).is_some(), "next auto id");
    }

    #[test]
    fn kitty_delete_id_drops_image_and_placements() {
        let mut pane = Pane::with_size(10, 5, 20);
        pane.feed(b"\x1b_Ga=T,i=3,f=24,w=1,h=1;AA==\x1b\\");
        assert_eq!(pane.graphics().placements().len(), 1);
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
    fn image_store_ids_are_process_unique() {
        // Issue #35: the renderer's bitmap cache keys on (store_id, id);
        // two panes (two tabs' first leaves) must never share a key.
        let a = ImageStore::default();
        let b = ImageStore::default();
        assert_ne!(a.store_id(), b.store_id());
    }

    #[test]
    fn feed_snaps_only_while_following() {
        // Issue #42: scrolling away from the live edge pauses the snap, so
        // a burst of output does not yank the reading position.
        let mut pane = Pane::with_size(10, 4, 50);
        pane.feed(b"l1\r\nl2\r\nl3\r\nl4\r\nl5\r\n");
        pane.update_follow();
        assert!(pane.follow(), "starts at the live edge");
        pane.grid_mut().scroll_up(2);
        pane.update_follow();
        assert!(!pane.follow(), "scrolled away pauses following");
        pane.feed(b"more output\r\n");
        assert_eq!(
            pane.grid().line_text(0).as_deref(),
            Some("l1"),
            "reading position holds"
        );
    }

    #[test]
    fn returning_to_live_edge_resumes_snapping() {
        // Issue #42: reaching the live edge (wheel down, jump) resumes.
        let mut pane = Pane::with_size(10, 4, 50);
        pane.feed(b"l1\r\nl2\r\nl3\r\nl4\r\nl5\r\n");
        pane.grid_mut().scroll_up(2);
        pane.update_follow();
        pane.grid_mut().scroll_down(2);
        pane.update_follow();
        assert!(pane.follow(), "live edge resumes following");
        pane.feed(b"more output\r\n");
        assert_eq!(pane.grid().scroll_offset(), 0, "view follows new output");
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
                display: true,
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

    /// Six labelled lines on a 3-row screen, viewport scrolled back two
    /// lines: the view shows `l2..l4` and the live screen `l4..l6`.
    fn scrolled_back_pane() -> Pane {
        let mut pane = Pane::with_size(10, 3, 100);
        pane.feed(b"l1\r\nl2\r\nl3\r\nl4\r\nl5\r\nl6");
        pane.grid_mut().scroll_up(2);
        pane.update_follow();
        pane
    }

    #[test]
    fn text_written_after_fit_grid_stays_at_the_top_when_the_window_grows() {
        // Break caught: a shrink pushes the empty top line into scrollback,
        // and a later grow brings it back above the banner.
        let mut pane = Pane::with_size(80, 24, 100);
        pane.fit_grid(77, 23);
        pane.feed(b"first\r\n");
        let _ = pane.resize(77, 25);
        assert_eq!(pane.grid().line_text(0).as_deref(), Some("first"));
    }

    #[test]
    fn bell_and_notification_ask_for_attention_until_seen() {
        // Break caught: a session asking for you and nothing showing it.
        let mut pane = Pane::with_size(40, 5, 10);
        assert!(!pane.needs_attention());
        pane.feed(b"\x07");
        assert!(pane.needs_attention());
        assert_eq!(pane.attention_message(), None);
        pane.mark_seen();
        assert!(!pane.needs_attention());
        pane.feed(b"\x1b]9;Approve\x1b[31m it?\x07");
        assert!(pane.needs_attention());
        pane.mark_seen();
        pane.feed(b"\x1b]777;notify;Claude;Needs input\x07");
        assert_eq!(pane.attention_message(), Some("Claude: Needs input"));
    }

    #[test]
    fn progress_is_tracked() {
        let mut pane = Pane::with_size(40, 5, 10);
        assert!(!pane.progress_active());
        pane.feed(b"\x1b]9;4;1;50\x07");
        assert!(pane.progress_active());
        assert!(!pane.needs_attention(), "progress is not a request");
        pane.feed(b"\x1b]9;4;0;0\x07");
        assert!(!pane.progress_active());
    }

    #[test]
    fn output_and_input_ages_are_tracked() {
        let mut pane = Pane::with_size(40, 5, 10);
        assert_eq!(pane.output_age(), None);
        assert_eq!(pane.input_age(), None);
        pane.feed(b"x");
        assert!(pane.output_age().is_some());
        let _ = pane.send_input(b"y");
        assert!(pane.input_age().is_some());
    }

    #[test]
    fn a_stopped_agent_shows_until_resumed_or_dismissed() {
        // Break caught: a restart silently losing a running agent.
        let mut pane = Pane::with_size(40, 5, 10);
        pane.set_interrupted(Some("claude".to_string()));
        assert_eq!(pane.rail_step(true, 0), crate::rail::Status::Interrupted);
        assert_eq!(pane.interrupted(), Some("claude"));
        let _ = pane.send_input(b"x");
        assert_eq!(pane.interrupted(), None, "typing dismisses it");

        pane.set_interrupted(Some("claude".to_string()));
        pane.set_agent(Some("claude"));
        assert_eq!(pane.interrupted(), None, "it is running again");
    }

    #[test]
    fn only_an_agent_with_a_known_folder_is_remembered() {
        // Break caught: `claude --continue` resuming a conversation from the
        // wrong folder.
        let mut pane = Pane::with_size(40, 5, 10);
        pane.set_agent(Some("claude"));
        assert_eq!(pane.saved_agent(), None);
        pane.feed(b"\x1b]7;file://DESKTOP/C:/src/rust\x07");
        assert_eq!(pane.saved_agent(), Some("claude"));
        pane.set_agent(None);
        assert_eq!(pane.saved_agent(), None);
        pane.set_interrupted(Some("codex".to_string()));
        assert_eq!(pane.saved_agent(), Some("codex"), "kept across restarts");
    }

    #[test]
    fn an_agents_own_folder_beats_what_the_shell_last_said() {
        // Break caught: PowerShell reports nothing, so a running agent was
        // forgotten; or the shell's old folder resumed the wrong conversation.
        let mut pane = Pane::with_size(40, 5, 10);
        pane.set_agent(Some("claude"));
        pane.set_process_cwd(Some("C:/work/api".to_string()));
        assert_eq!(pane.saved_cwd(), Some("C:/work/api"));
        assert_eq!(pane.saved_agent(), Some("claude"));
        pane.feed(b"\x1b]7;file://DESKTOP/C:/old\x07");
        assert_eq!(pane.saved_cwd(), Some("C:/work/api"));
        pane.set_agent(None);
        pane.set_process_cwd(Some("C:/shell/start".to_string()));
        assert_eq!(
            pane.saved_cwd(),
            Some("C:/old"),
            "no agent: the shell's report wins"
        );
    }

    #[test]
    fn a_program_saying_it_waits_shows_as_needs_you_even_while_viewed() {
        // Break caught: Claude's trust prompt reading as "Ready".
        use crate::rail::Status;
        let mut pane = Pane::with_size(40, 5, 10);
        // base64 of "Approve?"
        pane.feed(b"\x1b]7501;state=blocked:kind=permission:msg=QXBwcm92ZT8=\x07");
        assert_eq!(pane.rail_step(true, 0), Status::NeedsYou);
        assert_eq!(pane.rail_step(false, 0), Status::NeedsYou);
        assert_eq!(
            pane.status_message(Status::NeedsYou).as_deref(),
            Some("Approve?")
        );
        pane.feed(b"\x1b]7501;state=blocked:kind=question\x07");
        assert_eq!(
            pane.status_message(Status::NeedsYou).as_deref(),
            Some("Has a question for you")
        );
    }

    #[test]
    fn a_report_beats_guessing_from_output() {
        // Break caught: a status bar redrawing making an idle agent "Working".
        use crate::rail::Status;
        let mut pane = Pane::with_size(40, 5, 10);
        pane.feed(b"\x1b]7501;state=idle\x07noise");
        assert_eq!(pane.rail_step(false, 0), Status::Idle);
        pane.feed(b"\x1b]7501;state=working\x07");
        assert_eq!(pane.rail_step(false, 10), Status::Working);
    }

    #[test]
    fn a_result_shows_until_seen_and_then_gives_way() {
        use crate::rail::Status;
        let mut pane = Pane::with_size(40, 5, 10);
        pane.feed(b"\x1b]7501;state=done\x07");
        assert_eq!(pane.rail_step(false, 0), Status::Finished);
        assert_eq!(
            pane.rail_step(true, 1),
            Status::Working,
            "falls back: output just arrived"
        );
        pane.feed(b"\x1b]7501;state=error\x07");
        assert_eq!(pane.rail_step(false, 2), Status::Failed);
    }

    #[test]
    fn an_agent_that_ended_is_not_working_any_more() {
        // Break caught: a killed agent showing Working forever.
        let mut pane = Pane::with_size(40, 5, 10);
        pane.set_agent(Some("claude"));
        pane.feed(b"\x1b]7501;state=working\x07");
        assert!(pane.program_status.summary().is_some());
        pane.set_agent(None);
        assert!(pane.program_status.summary().is_none());
        pane.feed(b"\x1b]7501;state=blocked\x07\x1b]133;A\x07");
        assert!(
            pane.program_status.summary().is_none(),
            "a new prompt ends it too"
        );
    }

    #[test]
    fn an_agents_question_on_screen_needs_you_until_it_is_answered() {
        use crate::rail::Status;
        let mut pane = Pane::with_size(60, 10, 10);
        pane.set_agent(Some("claude"));
        pane.feed(
            "Do you want to create a.txt?\r\n\u{276F} 1. Yes\r\n  2. No\r\nEsc to cancel"
                .as_bytes(),
        );
        assert_eq!(
            pane.rail_step(true, 0),
            Status::NeedsYou,
            "even while viewed"
        );
        assert_eq!(
            pane.status_message(Status::NeedsYou).as_deref(),
            Some("Do you want to create a.txt?")
        );
        pane.feed(b"\x1b[2J\x1b[H");
        assert_ne!(pane.rail_step(true, 1), Status::NeedsYou);
        // Not an agent: those words mean nothing.
        let mut shell = Pane::with_size(60, 10, 10);
        shell.feed("Do you want to create a.txt?\r\n1. Yes\r\nEsc to cancel".as_bytes());
        assert_ne!(shell.rail_step(false, 0), Status::NeedsYou);
    }

    #[test]
    fn a_redrawing_status_bar_is_not_an_agent_working() {
        // Break caught: Claude showing Working long after it finished, and
        // Finished without doing anything, because its status line redraws.
        use crate::rail::Status;
        let mut pane = Pane::with_size(60, 10, 10);
        pane.set_agent(Some("claude"));
        pane.feed(b"\x1b]0;\xE2\x9C\xB3 Done\x07status bar redraw");
        assert_eq!(pane.rail_step(false, 0), Status::Idle);
        pane.feed(b"\x1b]0;\xE2\xA0\x82 Busy\x07");
        assert_eq!(pane.rail_step(false, 10), Status::Working);
    }

    #[test]
    fn a_prompt_directory_report_sets_the_folder() {
        // OSC 9;9 is how many Windows prompts report where they are.
        let mut pane = Pane::with_size(40, 5, 10);
        pane.feed(b"\x1b]9;9;\"C:\\src\\rust\"\x07");
        assert_eq!(pane.cwd(), Some("C:/src/rust"));
        assert!(!pane.needs_attention());
    }

    #[test]
    fn a_new_prompt_ends_progress() {
        // Break caught: a program killed mid-progress showing Working forever.
        let mut pane = Pane::with_size(40, 5, 10);
        pane.feed(b"\x1b]9;4;1;50\x07");
        assert!(pane.progress_active());
        pane.feed(b"\x1b]133;A\x07");
        assert!(!pane.progress_active());
    }

    #[test]
    fn a_resize_repaint_does_not_look_like_work() {
        // Break caught: dragging the window marking every session Working.
        let mut pane = Pane::with_size(40, 5, 10);
        let _ = pane.resize(50, 6);
        pane.feed(b"repaint");
        assert_eq!(pane.output_age(), None);
    }

    #[test]
    fn fit_grid_resizes_without_a_shell() {
        let mut pane = Pane::with_size(80, 24, 100);
        pane.fit_grid(100, 30);
        assert_eq!((pane.grid().cols, pane.grid().rows), (100, 30));
    }

    #[test]
    fn line_edits_while_scrolled_back_hit_the_live_screen() {
        // Break caught: erase/ICH/DCH/ECH resolving through the scrolled
        // viewport, which destroys history and leaves the live row stale.
        let cases: [(&str, &[u8], &str); 4] = [
            ("EL", b"\x1b[1;1H\x1b[2K", ""),
            ("DCH", b"\x1b[1;1H\x1b[P", "4"),
            ("ICH", b"\x1b[1;1H\x1b[@", " l4"),
            ("ECH", b"\x1b[1;1H\x1b[X", " 4"),
        ];
        for (name, seq, live_row) in cases {
            let mut pane = scrolled_back_pane();
            pane.feed(seq);
            assert_eq!(
                pane.grid().line_text_abs(1).as_deref(),
                Some("l2"),
                "{name} must not touch the history row on screen"
            );
            assert_eq!(
                pane.grid().line_text_abs(3).as_deref(),
                Some(live_row),
                "{name} must edit the live row"
            );
        }
    }

    #[test]
    fn line_edits_bump_the_search_revision_only_when_text_changes() {
        // Break caught: search highlights going stale after an edit, or
        // ordinary `text CSI K` output restarting search scans (issue #107).
        let cases: [(&str, &[u8], bool); 6] = [
            ("EL over text", b"\x1b[1;1H\x1b[K", true),
            ("EL past text", b"\x1b[1;4H\x1b[K", false),
            ("DCH over text", b"\x1b[1;1H\x1b[P", true),
            ("DCH on blank row", b"\x1b[2;1H\x1b[P", false),
            ("ICH over text", b"\x1b[1;1H\x1b[@", true),
            ("ECH on blank row", b"\x1b[2;1H\x1b[X", false),
        ];
        for (name, seq, bumps) in cases {
            let mut pane = Pane::with_size(10, 3, 100);
            pane.feed(b"abc");
            let before = pane.grid().content_rev();
            pane.feed(seq);
            assert_eq!(pane.grid().content_rev() != before, bumps, "{name}");
        }
    }

    #[test]
    fn scrolled_back_view_keeps_its_text_when_output_evicts_history() {
        // Break caught: the view advancing with each appended line, or
        // sliding when the full buffer evicts its oldest line.
        let mut pane = Pane::with_size(10, 3, 10);
        for i in 0..20 {
            pane.feed(format!("line{i}\r\n").as_bytes());
        }
        pane.grid_mut().scroll_up(5);
        pane.update_follow();
        assert_eq!(pane.grid().line_text(0).as_deref(), Some("line13"));
        for i in 20..23 {
            pane.feed(format!("line{i}\r\n").as_bytes());
        }
        assert_eq!(pane.grid().line_text(0).as_deref(), Some("line13"));
    }

    #[test]
    fn scrolled_back_view_stops_at_the_oldest_line_once_its_text_is_evicted() {
        // Break caught: the offset running past the buffer once the line
        // being read is gone.
        let mut pane = Pane::with_size(10, 3, 10);
        for i in 0..20 {
            pane.feed(format!("line{i}\r\n").as_bytes());
        }
        pane.grid_mut().scroll_up(5);
        pane.update_follow();
        for i in 20..33 {
            pane.feed(format!("line{i}\r\n").as_bytes());
        }
        assert_eq!(pane.grid().scroll_offset(), 10);
        assert_eq!(pane.grid().line_text(0).as_deref(), Some("line21"));
    }

    #[test]
    fn alt_screen_output_keeps_main_screen_commands() {
        // Break caught: alt-grid line ids retiring commands that the main
        // grid still holds.
        let mut pane = Pane::with_size(10, 3, 100);
        pane.feed(b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07out\r\n\x1b]133;D:0\x07");
        assert_eq!(pane.commands().len(), 1);
        pane.feed(b"\x1b[?1049h");
        for _ in 0..200 {
            pane.feed(b"x\r\n");
        }
        pane.feed(b"\x1b[?1049l");
        assert_eq!(pane.commands().len(), 1);
    }

    #[test]
    fn clear_scrollback_inside_alt_screen_keeps_main_screen_commands() {
        // Break caught: ED 3 in a full-screen app retiring main-grid commands.
        let mut pane = Pane::with_size(10, 3, 100);
        pane.feed(b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07out\r\n\x1b]133;D:0\x07");
        pane.feed(b"\x1b[?1049h");
        for _ in 0..200 {
            pane.feed(b"x\r\n");
        }
        pane.feed(b"\x1b[3J\x1b[?1049l");
        assert_eq!(pane.commands().len(), 1);
    }

    #[test]
    fn main_screen_eviction_still_retires_commands() {
        // Break caught: guarding retirement so broadly that evicted main
        // history keeps commands forever (SHELLINT-021).
        let mut pane = Pane::with_size(10, 3, 5);
        pane.feed(b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07out\r\n\x1b]133;D:0\x07");
        assert_eq!(pane.commands().len(), 1);
        for _ in 0..50 {
            pane.feed(b"x\r\n");
        }
        assert_eq!(pane.commands().len(), 0);
    }

    #[test]
    fn reset_then_text_in_one_chunk_scrolls_normally() {
        // Break caught: parsing past `ESC c` with the scroll region unset,
        // which makes every following line overwrite the last row.
        let mut pane = Pane::with_size(10, 3, 100);
        pane.feed(b"\x1bca\r\nb\r\nc\r\nd\r\ne");
        let rows: Vec<_> = (0..3).map(|r| pane.grid().line_text(r)).collect();
        assert_eq!(
            rows,
            [
                Some("c".to_string()),
                Some("d".to_string()),
                Some("e".to_string())
            ]
        );
    }

    #[test]
    fn compound_alt_screen_mode_places_following_text_on_the_alt_screen() {
        // Break caught: a later mode in the same CSI hiding the grid switch,
        // so the chunk's text lands on the main screen (issue #1).
        let mut pane = Pane::with_size(10, 3, 100);
        pane.feed(b"main");
        pane.feed(b"\x1b[?1049;25hALT");
        assert_eq!(pane.grid().line_text(0).as_deref(), Some("ALT"));
        pane.feed(b"\x1b[?1049l");
        assert_eq!(pane.grid().line_text(0).as_deref(), Some("main"));
    }

    #[test]
    fn full_screen_region_follows_a_later_grow() {
        // Break caught: `CSI r` pinning the region at the old height, so
        // output never reaches the new rows or the history (issue #3).
        let cases: [(&str, &[u8]); 2] =
            [("CSI r", b"\x1b[r"), ("explicit full height", b"\x1b[1;3r")];
        for (name, seq) in cases {
            let mut pane = Pane::with_size(10, 3, 100);
            pane.feed(seq);
            let _ = pane.resize(10, 5);
            pane.feed(b"\x1b[5;1H\r\nX");
            assert_eq!(pane.parser().cursor_row, 4, "{name}");
            assert_eq!(pane.grid().line_text(4).as_deref(), Some("X"), "{name}");
        }
    }

    #[test]
    fn partial_region_stays_put_across_a_grow() {
        // Break caught: treating every DECSTBM as following the bottom.
        let mut pane = Pane::with_size(10, 3, 100);
        pane.feed(b"\x1b[1;2r");
        let _ = pane.resize(10, 5);
        pane.feed(b"x");
        assert_eq!(pane.parser().scroll_bottom, 1);
    }

    #[test]
    fn image_placed_while_scrolled_back_anchors_to_the_live_cursor() {
        // Break caught: the anchor resolving the cursor row through the
        // scrolled viewport, pinning the image to a history line.
        let mut pane = scrolled_back_pane();
        pane.feed(b"\x1b_Ga=t,i=7;YQ==\x1b\\");
        pane.feed(b"\x1b[1;1H\x1b_Ga=p,i=7\x1b\\");
        assert_eq!(pane.graphics().placements()[0].top_abs, 3);
    }
}
