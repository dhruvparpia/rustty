//! VT/ANSI/VT100+ parser: turns shell output bytes into grid writes.
//!
//! One [`Parser`] belongs to one pane and owns the terminal state the grid
//! does not carry: cursor position, SGR pen, scroll region, saved cursor,
//! DEC modes. It is a streaming state machine — partial UTF-8 sequences are
//! buffered across [`Parser::advance`] calls, which is why ConPTY bytes must
//! reach it byte-for-byte (see `conpty.rs`).
//!
//! Out of MVP scope (spec §4): OSC 4/10/11 palette writes, OSC 52
//! clipboard, soft reset (`CSI !p`). Mouse tracking is decoded for
//! reporting modes `1000`, `1002`, `1003`, `1006`, and `1015`
//! (MOUSE-009..012). OSC 8 hyperlinks are supported (issue #41): the
//! link id rides in each cell, and the URI lives in a bounded table
//! owned by this parser.

use std::collections::HashMap;

use crate::color::{Color, ansi_palette};
use crate::graphics::{GraphicsAction, parse_kitty};
use crate::grid::{Cell, CellAttribute, Grid};

/// Side effects the pane must apply after a [`Parser::advance`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParserEvent {
    /// OSC 0/1/2 requested a tab title.
    Title(String),
    /// The shell asked for the alternate screen (`CSI ?1049h` family).
    EnterAltScreen,
    /// The shell left the alternate screen (`CSI ?1049l` family).
    ExitAltScreen,
    /// `CSI ?25 h/l`: draw or hide the cursor.
    CursorVisibility(bool),
    /// `ED 3` / `ESC c`: drop scrollback; the pane rebuilds its grid.
    ClearScrollback,
    /// Ready-made reply bytes to hand back to the shell (DSR, DA).
    Respond(Vec<u8>),
    /// OSC 7: the shell reports its working directory
    /// (`file://host/path`, percent-encoded).
    Cwd(String),
    /// OSC 8: the hyperlink pen opened (`Some(uri)`) or closed
    /// (`None`) (issue #41, PARSER-028).
    Hyperlink(Option<String>),
    /// OSC 133;A: the shell is sitting at a prompt.
    PromptStart,
    /// OSC 133;B: a command was accepted and is about to run.
    CommandStart,
    /// OSC 133;C: the command produced its first output.
    CommandOutput,
    /// OSC 133;D[:`exit`]: the command finished with this exit code.
    PromptEnd(Option<i32>),
    /// BEL: the shell rang the bell (BELL-001).
    Bell,
    /// Kitty graphics command (`ESC _ G ...` / `ESC P G ...`, roadmap v0.5).
    Graphics(GraphicsAction),
}

/// What one [`Parser::advance`] call produced.
#[derive(Debug, Clone, Default)]
pub struct AdvanceOutcome {
    /// Events the pane must apply, in emission order.
    pub events: Vec<ParserEvent>,
    /// True when the bytes produced visible output (drives scroll snap).
    pub printed: bool,
    /// Bytes consumed from the input; stops early after a grid-changing
    /// event (issue #1, #7).
    pub consumed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Streaming VT state-machine state.
pub enum State {
    /// Ordinary printable output state.
    Ground,
    /// After ESC, before a final byte.
    Escape,
    /// ESC plus intermediate bytes.
    EscapeIntermediate,
    /// Inside a CSI sequence.
    Csi,
    /// Inside an OSC string.
    Osc,
    /// OSC string with an ESC byte seen.
    OscEscape,
    /// Inside a kitty graphics string, after `ESC _`/`ESC P`/0x90.
    KittyDetect,
    /// Inside a kitty graphics string, collecting the payload.
    Kitty,
    /// Kitty string saw `ESC`: next byte is the ST (`\`) or payload data.
    KittyEscape,
    /// Inside an ignored string.
    Ignore,
    /// Ignored string with an ESC byte seen.
    IgnoreEscape,
}

#[derive(Debug, Clone, Copy)]
/// Saved cursor snapshot for CSI 8/ and DEC save/restore.
pub struct SavedCursor {
    /// Saved viewport row.
    pub row: usize,
    /// Saved viewport column.
    pub col: usize,
    /// Saved foreground pen.
    pub fg: Option<Color>,
    /// Saved background pen.
    pub bg: Option<Color>,
    /// Saved SGR attributes.
    pub attrs: CellAttribute,
}

/// Mouse reporting protocol requested by the shell (MOUSE-009).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseProtocol {
    /// Mouse reporting disabled.
    #[default]
    Off,
    /// `1000`: button press/release.
    Click,
    /// `1002`: button press/release plus drags.
    Drag,
    /// `1003`: button press/release plus all motion.
    Any,
}

/// Streaming VT parser plus the terminal state for one pane.
#[derive(Debug, Clone)]
pub struct Parser {
    /// Current state-machine state.
    pub state: State,
    /// CSI parameter buffer.
    pub params: Vec<u8>,
    /// CSI private-marker buffer.
    pub private: Vec<u8>,
    /// CSI intermediate buffer.
    pub intermediates: Vec<u8>,
    /// OSC payload buffer.
    pub osc: Vec<u8>,
    /// Buffered partial UTF-8 bytes.
    pub utf8_pending: Vec<u8>,
    /// Cursor row in viewport coordinates.
    pub cursor_row: usize,
    /// Cursor column in viewport coordinates.
    pub cursor_col: usize,
    /// `CSI ?25`: cursor is drawn.
    pub cursor_visible: bool,
    /// `CSI ?7`: auto-wrap mode (DECAWM).
    pub auto_wrap: bool,
    /// `CSI ?2004`: bracketed-paste mode (the keyboard layer wraps pastes).
    pub bracketed_paste: bool,
    /// `CSI ?1`: application cursor keys (the keyboard layer encodes keys).
    pub application_cursor_keys: bool,
    /// `CSI ?1000/?1002/?1003`: requested mouse reporting protocol.
    pub mouse_protocol: MouseProtocol,
    /// `CSI ?1006`: SGR coordinate mouse encoding.
    pub mouse_sgr_coordinates: bool,
    /// `CSI ?1015`: urxvt coordinate mouse encoding.
    pub mouse_urxvt_coordinates: bool,
    /// Top of the scroll region.
    pub scroll_top: usize,
    /// `usize::MAX` until the first grid contact; [`Parser::fit`] clamps it.
    pub scroll_bottom: usize,
    /// Issue #3: no `DECSTBM` has run, so the scroll region's bottom edge
    /// follows the screen bottom across resize (grow restores full-height
    /// scrolling).
    pub region_follows_bottom: bool,
    /// #22: bytes of a UTF-8 sequence still expected inside a string state
    /// (OSC / DCS), where a 0x9C continuation byte is data, not ST.
    pub string_utf8: u8,
    /// Saved cursor, if any.
    pub saved: Option<SavedCursor>,
    /// Current foreground pen.
    pub pen_fg: Option<Color>,
    /// Current background pen.
    pub pen_bg: Option<Color>,
    /// Current SGR attributes.
    pub pen_attrs: CellAttribute,
    /// OSC 8 link pen: id into `link_uris`, `0` when no link is open.
    pub pen_link: u32,
    /// OSC 8 URI table; id = index + 1, so id `0` means "no link".
    /// Bounded: past `LINK_CAP` new URIs get no target (their cells
    /// render as links with nothing to open/copy); already-interned
    /// targets keep working.
    pub link_uris: Vec<String>,
    /// URI intern table.
    pub link_ids: HashMap<String, u32>,
    /// Pending auto-wrap at the right edge.
    pub wrap_pending: bool,
    /// Last printed base cell column, used for combining marks (issue #112).
    pub last_base_col: Option<usize>,
}

/// OSC 8 URI table size limit (issue #41). Typical panes open a handful
/// of links; refusing new targets past this bound keeps memory flat
/// against a hostile stream. shortcut: new links die at the cap,
/// upgrade to LRU eviction if a real workload ever hits it.
pub(crate) const LINK_CAP: usize = 1024;

impl Parser {
    /// Create a parser with default terminal state (visible cursor,
    /// auto-wrap on, full scroll region).
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            params: Vec::new(),
            private: Vec::new(),
            intermediates: Vec::new(),
            osc: Vec::new(),
            utf8_pending: Vec::new(),
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            auto_wrap: true,
            bracketed_paste: false,
            application_cursor_keys: false,
            mouse_protocol: MouseProtocol::Off,
            mouse_sgr_coordinates: false,
            mouse_urxvt_coordinates: false,
            scroll_top: 0,
            scroll_bottom: usize::MAX,
            region_follows_bottom: true,
            string_utf8: 0,
            saved: None,
            pen_fg: None,
            pen_bg: None,
            pen_attrs: CellAttribute::empty(),
            pen_link: 0,
            link_uris: Vec::new(),
            link_ids: HashMap::new(),
            wrap_pending: false,
            last_base_col: None,
        }
    }

    /// Feed raw shell output bytes, writing through `grid`.
    ///
    /// The outcome reports how many bytes were consumed. When an event
    /// changes which grid the pane draws into (alt-screen enter/exit,
    /// scrollback clear), `advance` stops after that event so the pane can
    /// swap grids before the next byte is interpreted (issue #1, #7):
    /// "everything a chunk prints is placed on the same screen the
    /// mode-switch event describes."
    pub fn advance(&mut self, grid: &mut Grid, bytes: &[u8]) -> AdvanceOutcome {
        self.fit(grid);
        let mut events = Vec::new();
        let mut printed = false;
        for (i, &b) in bytes.iter().enumerate() {
            // Mid-multibyte-UTF-8, continuation bytes 0x80..=0xBF are
            // *not* C1 controls (0x9F is a legal continuation byte, e.g.
            // U+1F600), so they bypass the state machine entirely.
            if self.state == State::Ground && !self.utf8_pending.is_empty() {
                match b {
                    // Continuation byte: append to the running sequence.
                    0x80..=0xBF => self.utf8_byte(grid, b, &mut printed),
                    // A new start byte before the old sequence finished:
                    // flush the partial bytes as one U+FFFD, start fresh.
                    0xC0..=0xF7 => {
                        self.utf8_pending.clear();
                        self.print_char(grid, '\u{FFFD}');
                        printed = true;
                        self.utf8_byte(grid, b, &mut printed);
                    }
                    // Issue #23: the truncated sequence flushes as one
                    // U+FFFD, and the byte that broke it is then
                    // interpreted normally: printable characters print,
                    // ESC starts its sequence, CAN/SUB cancel.
                    _ => {
                        self.utf8_pending.clear();
                        self.print_char(grid, '\u{FFFD}');
                        printed = true;
                    }
                }
                if matches!(b, 0x80..=0xF7) {
                    continue;
                }
            }
            match self.state {
                State::Ground => self.ground_byte(grid, b, &mut events, &mut printed),
                State::Escape => match b {
                    0x1B => {}
                    0x5B => self.begin_csi(),
                    0x5D => {
                        self.osc.clear();
                        self.string_utf8 = 0;
                        self.state = State::Osc;
                    }
                    0x50 | 0x5F => self.state = State::KittyDetect,
                    0x7D => self.state = State::Ignore,
                    0x20..=0x2F => self.state = State::EscapeIntermediate,
                    // ECMA-48 lets 0x30..=0x3F close an escape sequence:
                    // ESC 7 / ESC 8 (DECSC / DECRC) live here.
                    0x30..=0x7E => {
                        self.dispatch_esc(grid, b, &mut events, &mut printed);
                        self.state = State::Ground;
                    }
                    _ => self.state = State::Ground,
                },
                State::EscapeIntermediate => self.state = State::Ground,
                State::Csi => match b {
                    0x40..=0x7E => {
                        self.dispatch_csi(grid, b, &mut events, &mut printed);
                        self.state = State::Ground;
                    }
                    // GHSA-845w-h489-83vg: the sequence buffers are capped;
                    // overflow bytes are consumed and dropped, and the
                    // sequence dispatches with truncated parameters.
                    0x30..=0x3B => {
                        if self.params.len() < 64 {
                            self.params.push(b);
                        }
                    }
                    0x3C..=0x3F
                        if self.params.is_empty()
                            && self.intermediates.is_empty()
                            && self.private.len() < 8 =>
                    {
                        self.private.push(b);
                    }
                    0x20..=0x2F => {
                        if self.intermediates.len() < 4 {
                            self.intermediates.push(b);
                        }
                    }
                    0x1B => self.state = State::Escape,
                    _ => {}
                },
                State::Osc => {
                    // #22: a 0x9C byte that is a UTF-8 continuation byte
                    // (mid-sequence) is OSC data, not the ST terminator;
                    // 0x07 always terminates.
                    if self.string_utf8 > 0 {
                        if (0x80..=0xBF).contains(&b) {
                            self.string_utf8 -= 1;
                            self.osc.push(b);
                            continue;
                        }
                        self.string_utf8 = 0;
                    }
                    match b {
                        0x07 | 0x9C => {
                            self.string_utf8 = 0;
                            self.finish_osc(&mut events);
                            self.state = State::Ground;
                        }
                        0x1B => self.state = State::OscEscape,
                        0xC2..=0xDF => {
                            self.string_utf8 = 1;
                            self.osc.push(b);
                        }
                        0xE0..=0xEF => {
                            self.string_utf8 = 2;
                            self.osc.push(b);
                        }
                        0xF0..=0xF7 => {
                            self.string_utf8 = 3;
                            self.osc.push(b);
                        }
                        _ => self.osc.push(b),
                    }
                    // GHSA-845w-h489-83vg: a never-terminated OSC consumes
                    // unbounded RAM; past the cap the payload is dropped
                    // and the string is skipped to its terminator.
                    if self.osc.len() >= 65_536 {
                        self.osc.clear();
                        self.string_utf8 = 0;
                        self.state = State::Ignore;
                    }
                }
                State::OscEscape => match b {
                    0x5C => {
                        self.string_utf8 = 0;
                        self.finish_osc(&mut events);
                        self.state = State::Ground;
                    }
                    0x1B => {}
                    _ => {
                        self.osc.push(0x1B);
                        self.osc.push(b);
                        self.state = State::Osc;
                    }
                },
                State::KittyDetect => match b {
                    b'G' => {
                        self.osc.clear();
                        self.string_utf8 = 0;
                        self.state = State::Kitty;
                    }
                    0x1B => self.state = State::Escape,
                    0x07 | 0x9C => self.state = State::Ground,
                    _ => self.state = State::Ignore,
                },
                State::Kitty => {
                    // #22 applies to the DCS string state as well.
                    if self.string_utf8 > 0 {
                        if (0x80..=0xBF).contains(&b) {
                            self.string_utf8 -= 1;
                            self.osc.push(b);
                            continue;
                        }
                        self.string_utf8 = 0;
                    }
                    match b {
                        0x1B => self.state = State::KittyEscape,
                        0x07 | 0x9C => {
                            self.string_utf8 = 0;
                            self.finish_kitty(&mut events);
                            self.state = State::Ground;
                        }
                        0xC2..=0xDF => {
                            self.string_utf8 = 1;
                            self.osc.push(b);
                        }
                        0xE0..=0xEF => {
                            self.string_utf8 = 2;
                            self.osc.push(b);
                        }
                        0xF0..=0xF7 => {
                            self.string_utf8 = 3;
                            self.osc.push(b);
                        }
                        _ => self.osc.push(b),
                    }
                    if self.osc.len() >= 65_536 {
                        self.osc.clear();
                        self.string_utf8 = 0;
                        self.state = State::Ignore;
                    }
                }
                State::KittyEscape => match b {
                    0x5C => {
                        self.string_utf8 = 0;
                        self.finish_kitty(&mut events);
                        self.state = State::Ground;
                    }
                    0x1B => {}
                    _ => {
                        self.osc.push(0x1B);
                        self.osc.push(b);
                        self.state = State::Kitty;
                    }
                },
                State::Ignore => match b {
                    0x1B => self.state = State::IgnoreEscape,
                    0x07 | 0x9C => self.state = State::Ground,
                    _ => {}
                },
                State::IgnoreEscape => match b {
                    0x5C => self.state = State::Ground,
                    0x1B => {}
                    _ => self.state = State::Escape,
                },
            }
            if matches!(
                events.last(),
                Some(
                    ParserEvent::EnterAltScreen
                        | ParserEvent::ExitAltScreen
                        | ParserEvent::ClearScrollback
                )
            ) {
                self.fit(grid);
                return AdvanceOutcome {
                    events,
                    printed,
                    consumed: i + 1,
                };
            }
        }
        self.fit(grid);
        AdvanceOutcome {
            events,
            printed,
            consumed: bytes.len(),
        }
    }

    /// Clamp cursor, scroll region and pen to the grid's current size.
    /// Call after every grid resize.
    pub fn fit(&mut self, grid: &Grid) {
        let rows = grid.rows;
        let cols = grid.cols;
        // Issue #3: until a DECSTBM sets a custom region, the region's
        // bottom follows the screen bottom, so a shrink/grow cycle
        // restores full-height scrolling instead of staying pinned at the
        // shrunk height.
        if self.region_follows_bottom {
            self.scroll_bottom = rows - 1;
        }
        self.scroll_bottom = self.scroll_bottom.min(rows - 1);
        self.scroll_top = self.scroll_top.min(self.scroll_bottom);
        self.cursor_row = self.cursor_row.min(rows - 1);
        self.cursor_col = self.cursor_col.min(cols - 1);
    }

    /// Full terminal reset (`ESC c`): state back to power-on defaults.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// The OSC 8 URI a cell's link id points to, `None` if the target
    /// was evicted by the table cap (issue #41).
    #[must_use]
    pub fn link_uri(&self, id: u32) -> Option<&str> {
        if id == 0 {
            return None;
        }
        self.link_uris
            .get(usize::try_from(id - 1).ok()?)
            .map(String::as_str)
    }

    /// Intern an OSC 8 URI, returning its stable id. Empty URIs are the
    /// caller's job (they close the link). Past the cap the id is a
    /// sentinel with no target (SEC-004: unbounded streams stay bounded).
    fn intern_link(&mut self, uri: &str) -> u32 {
        if let Some(&id) = self.link_ids.get(uri) {
            return id;
        }
        if self.link_uris.len() >= LINK_CAP {
            return u32::MAX;
        }
        let id = u32::try_from(self.link_uris.len() + 1).unwrap_or(1);
        self.link_uris.push(uri.to_string());
        self.link_ids.insert(uri.to_string(), id);
        id
    }

    fn begin_csi(&mut self) {
        self.params.clear();
        self.private.clear();
        self.intermediates.clear();
        self.state = State::Csi;
    }

    fn ground_byte(
        &mut self,
        grid: &mut Grid,
        b: u8,
        events: &mut Vec<ParserEvent>,
        printed: &mut bool,
    ) {
        match b {
            0x1B => self.state = State::Escape,
            0x08 => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                }
                self.wrap_pending = false;
            }
            0x09 => {
                let next = (self.cursor_col / 8 + 1) * 8;
                self.cursor_col = next.min(grid.cols - 1);
                self.wrap_pending = false;
            }
            0x0A..=0x0C => self.line_feed(grid, printed),
            0x0D => {
                self.cursor_col = 0;
                self.wrap_pending = false;
                self.last_base_col = None;
            }
            0x07 => events.push(ParserEvent::Bell),
            0x00 | 0x0E..=0x1A | 0x1C..=0x1F | 0x7F => {}
            0x80..=0x9F => match b {
                0x84 => self.line_feed(grid, printed),
                0x85 => {
                    self.cursor_col = 0;
                    self.line_feed(grid, printed);
                }
                0x9B => self.begin_csi(),
                0x9D => {
                    self.osc.clear();
                    self.state = State::Osc;
                }
                0x90 => self.state = State::KittyDetect,
                0x98 | 0x9E | 0x9F => self.state = State::Ignore,
                0x9C => {
                    self.finish_osc(events);
                    self.state = State::Ground;
                }
                _ => {}
            },
            0x20..=0x7E => {
                self.print_char(grid, b as char);
                *printed = true;
            }
            _ => self.utf8_byte(grid, b, printed),
        }
    }

    fn is_combining(cp: u32) -> bool {
        (0x0300..=0x036F).contains(&cp)
            || (0x1AB0..=0x1AFF).contains(&cp)
            || (0x1DC0..=0x1DFF).contains(&cp)
            || (0x20D0..=0x20FF).contains(&cp)
            || (0xFE20..=0xFE2F).contains(&cp)
            || (0xFE00..=0xFE0F).contains(&cp)
    }

    fn is_wide(cp: u32) -> bool {
        (0x1100..=0x115F).contains(&cp)
            || (0x2E80..=0xA4CF).contains(&cp)
            || (0xA960..=0xA97F).contains(&cp)
            || (0xAC00..=0xD7AF).contains(&cp)
            || (0xF900..=0xFAFF).contains(&cp)
            || (0xFE10..=0xFE6F).contains(&cp)
            || (0xFF00..=0xFF60).contains(&cp)
            || (0xFFE0..=0xFFE6).contains(&cp)
            || (0x1F300..=0x1F64F).contains(&cp)
            || (0x1F900..=0x1F9FF).contains(&cp)
    }

    fn char_width(ch: char) -> usize {
        let cp = u32::from(ch);
        if Parser::is_combining(cp) {
            return 0;
        }
        if Parser::is_wide(cp) {
            return 2;
        }
        1
    }

    fn print_char(&mut self, grid: &mut Grid, ch: char) {
        let cols = grid.cols;
        let width = Parser::char_width(ch);
        if width == 0 {
            let Some(base_col) = self.last_base_col else {
                return;
            };
            let Some(cell) = grid.live_cell_mut(self.cursor_row, base_col) else {
                return;
            };
            let existing = cell
                .combining
                .clone()
                .map_or(String::new(), |s| s.to_string());
            let _ =
                grid.write_combining(self.cursor_row, base_col, Some(existing + &ch.to_string()));
            return;
        }
        if self.wrap_pending {
            self.wrap_pending = false;
            self.cursor_col = 0;
            self.line_feed_quiet(grid);
        }
        if width == 2 && self.cursor_col + 1 >= cols && self.auto_wrap {
            self.wrap_pending = false;
            self.cursor_col = 0;
            self.line_feed_quiet(grid);
        }
        self.last_base_col = Some(self.cursor_col);
        let cell = Cell {
            char: ch,
            fg: self.pen_fg,
            bg: self.pen_bg,
            attributes: self.pen_attrs,
            link: self.pen_link,
            width: u8::try_from(width).unwrap_or(1),
            continuation: false,
            combining: None,
        };
        grid.write_cell(self.cursor_row, self.cursor_col, cell);
        if width == 2 && self.cursor_col + 1 < cols {
            let cont = Cell {
                char: ' ',
                fg: self.pen_fg,
                bg: self.pen_bg,
                attributes: self.pen_attrs,
                link: self.pen_link,
                width: 1,
                continuation: true,
                combining: None,
            };
            grid.write_cell(self.cursor_row, self.cursor_col + 1, cont);
            self.cursor_col += 2;
            if self.cursor_col >= cols {
                if self.auto_wrap {
                    self.wrap_pending = true;
                } else {
                    self.cursor_col = cols - 1;
                }
            }
        } else if width == 1 {
            if self.cursor_col + 1 < cols {
                self.cursor_col += 1;
            } else if self.auto_wrap {
                self.wrap_pending = true;
            }
        } else {
            if self.cursor_col + 1 >= cols && self.auto_wrap {
                self.wrap_pending = true;
            }
        }
    }

    fn line_feed(&mut self, grid: &mut Grid, printed: &mut bool) {
        self.line_feed_quiet(grid);
        *printed = true;
    }

    fn line_feed_quiet(&mut self, grid: &mut Grid) {
        self.wrap_pending = false;
        self.last_base_col = None;
        if self.cursor_row >= self.scroll_bottom {
            grid.scroll_region_up(self.scroll_top, self.scroll_bottom);
            self.cursor_row = self.scroll_bottom;
        } else {
            self.cursor_row += 1;
        }
    }

    fn snapshot(&self) -> SavedCursor {
        SavedCursor {
            row: self.cursor_row,
            col: self.cursor_col,
            fg: self.pen_fg,
            bg: self.pen_bg,
            attrs: self.pen_attrs,
        }
    }

    fn restore_saved(&mut self) {
        if let Some(s) = self.saved {
            self.cursor_row = s.row;
            self.cursor_col = s.col;
            self.pen_fg = s.fg;
            self.pen_bg = s.bg;
            self.pen_attrs = s.attrs;
            self.wrap_pending = false;
        }
    }

    fn alt_home(&mut self) {
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.scroll_top = 0;
        self.scroll_bottom = usize::MAX;
        self.region_follows_bottom = true;
        self.wrap_pending = false;
    }

    /// A kitty graphics string ended: decode the payload into an event.
    fn finish_kitty(&mut self, events: &mut Vec<ParserEvent>) {
        let payload = std::mem::take(&mut self.osc);
        if let Some(action) = parse_kitty(&payload) {
            events.push(ParserEvent::Graphics(action));
        }
    }

    fn finish_osc(&mut self, events: &mut Vec<ParserEvent>) {
        let payload = std::mem::take(&mut self.osc);
        let text = String::from_utf8_lossy(&payload).into_owned();
        if let Some((prefix, rest)) = text.split_once(';') {
            match prefix {
                "0" | "1" | "2" => events.push(ParserEvent::Title(rest.to_string())),
                "7" => events.push(ParserEvent::Cwd(rest.to_string())),
                "8" => {
                    // OSC 8 `params;uri`; params (`id=`) are ignored. An
                    // empty URI closes the link. The target is stored as
                    // sent; validation happens at open time (SEC-004).
                    let uri = rest.split_once(';').map_or("", |(_, u)| u);
                    if uri.is_empty() {
                        self.pen_link = 0;
                        events.push(ParserEvent::Hyperlink(None));
                    } else {
                        let id = self.intern_link(uri);
                        self.pen_link = id;
                        events.push(ParserEvent::Hyperlink(Some(uri.to_string())));
                    }
                }
                "133" => {
                    let (marker, code) = match rest.split_once(':') {
                        Some((m, c)) => (m, c.parse().ok()),
                        None => (rest, None),
                    };
                    match marker {
                        "A" => events.push(ParserEvent::PromptStart),
                        "B" => events.push(ParserEvent::CommandStart),
                        "C" => events.push(ParserEvent::CommandOutput),
                        "D" => events.push(ParserEvent::PromptEnd(code)),
                        _ => {}
                    }
                }
                _ => {}
            }
        } else if !text.is_empty() && !text.chars().all(|c| c.is_ascii_digit()) {
            events.push(ParserEvent::Title(text));
        }
    }

    fn param_list(&self) -> Vec<u32> {
        self.param_slots().into_iter().map(|(v, _)| v).collect()
    }

    /// Parameters with separator identity: each value plus whether the
    /// separator that ended it was a colon (issue #4: SGR `38:2::r:g:b`
    /// must be told apart from `38;2;r;g;b`).
    fn param_slots(&self) -> Vec<(u32, bool)> {
        let mut out = Vec::new();
        let mut cur: u32 = 0;
        let mut digit = false;
        for &b in &self.params {
            match b {
                b'0'..=b'9' => {
                    cur = cur.saturating_mul(10).saturating_add(u32::from(b - b'0'));
                    digit = true;
                }
                b':' => {
                    out.push((if digit { cur } else { 0 }, true));
                    cur = 0;
                    digit = false;
                }
                _ => {
                    out.push((if digit { cur } else { 0 }, false));
                    cur = 0;
                    digit = false;
                }
            }
        }
        out.push((if digit { cur } else { 0 }, false));
        out
    }

    fn erase_cells(&mut self, grid: &mut Grid, row: usize, from: usize, to: usize) {
        if let Some(line) = grid.row_mut(row) {
            let last = to.min(line.cells.len().saturating_sub(1));
            if from <= last {
                for cell in &mut line.cells[from..=last] {
                    *cell = Cell::blank();
                }
            }
        }
    }

    fn dispatch_esc(
        &mut self,
        grid: &mut Grid,
        b: u8,
        events: &mut Vec<ParserEvent>,
        printed: &mut bool,
    ) {
        match b {
            b'7' => self.saved = Some(self.snapshot()),
            b'8' => self.restore_saved(),
            b'D' => self.line_feed(grid, printed),
            b'E' => {
                self.cursor_col = 0;
                self.line_feed(grid, printed);
            }
            b'M' => {
                // #6: RI moves the cursor up; only at the region top does
                // it scroll the region down. It never scrolls the page up
                // and never grows the scrollback offset.
                if self.cursor_row > self.scroll_top {
                    self.cursor_row -= 1;
                } else {
                    grid.scroll_region_down(self.scroll_top, self.scroll_bottom);
                }
                *printed = true;
            }
            b'c' => {
                self.reset();
                // Full reset clears the screen as well as the history
                // (ClearScrollback alone keeps the viewport).
                let rows = grid.rows;
                grid.erase_rows(0, rows - 1);
                events.push(ParserEvent::ClearScrollback);
                events.push(ParserEvent::CursorVisibility(true));
            }
            _ => {}
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the VT dispatcher reads best as one match"
    )]
    fn dispatch_csi(
        &mut self,
        grid: &mut Grid,
        final_byte: u8,
        events: &mut Vec<ParserEvent>,
        printed: &mut bool,
    ) {
        if !self.intermediates.is_empty() {
            return;
        }
        let params = self.param_list();
        let p0 = params.first().copied().unwrap_or(0);
        let n = usize::try_from(p0).unwrap_or(0).max(1);
        let rows = grid.rows;
        let cols = grid.cols;
        match final_byte {
            b'A' => {
                self.cursor_row = self.cursor_row.saturating_sub(n);
                self.wrap_pending = false;
            }
            b'B' => {
                // #5: CUD moves the cursor down; at the region bottom it
                // stops. Only a line feed scrolls (xterm/WSLg semantics:
                // "Cursor Down at the bottom of the scroll region must
                // scroll, Cursor Down in the middle must not").
                self.cursor_row = (self.cursor_row + n).min(self.scroll_bottom);
                self.wrap_pending = false;
            }
            b'C' => {
                self.cursor_col = (self.cursor_col + n).min(cols - 1);
                self.wrap_pending = false;
            }
            b'D' => {
                self.cursor_col = self.cursor_col.saturating_sub(n);
                self.wrap_pending = false;
            }
            b'E' => {
                self.cursor_row = (self.cursor_row + n).min(rows - 1);
                self.cursor_col = 0;
                self.wrap_pending = false;
            }
            b'F' => {
                self.cursor_row = self.cursor_row.saturating_sub(n);
                self.cursor_col = 0;
                self.wrap_pending = false;
            }
            b'G' => {
                self.cursor_col = usize::try_from(p0.saturating_sub(1))
                    .unwrap_or(0)
                    .min(cols - 1);
                self.wrap_pending = false;
            }
            b'd' => {
                self.cursor_row = usize::try_from(p0.saturating_sub(1))
                    .unwrap_or(0)
                    .min(rows - 1);
                self.wrap_pending = false;
            }
            b'H' | b'f' => {
                self.cursor_row = usize::try_from(p0.saturating_sub(1))
                    .unwrap_or(0)
                    .min(rows - 1);
                let p1 = params.get(1).copied().unwrap_or(1);
                self.cursor_col = usize::try_from(p1.saturating_sub(1))
                    .unwrap_or(0)
                    .min(cols - 1);
                self.wrap_pending = false;
            }
            b'J' => match p0 {
                0 => {
                    self.erase_cells(grid, self.cursor_row, self.cursor_col, cols - 1);
                    if self.cursor_row + 1 < rows {
                        grid.erase_rows(self.cursor_row + 1, rows - 1);
                    }
                    *printed = true;
                }
                1 => {
                    if self.cursor_row > 0 {
                        grid.erase_rows(0, self.cursor_row - 1);
                    }
                    self.erase_cells(grid, self.cursor_row, 0, self.cursor_col);
                    *printed = true;
                }
                2 | 12 => {
                    grid.erase_rows(0, rows - 1);
                    *printed = true;
                }
                3 => events.push(ParserEvent::ClearScrollback),
                _ => {}
            },
            b'K' => {
                match p0 {
                    0 => self.erase_cells(grid, self.cursor_row, self.cursor_col, cols - 1),
                    1 => self.erase_cells(grid, self.cursor_row, 0, self.cursor_col),
                    2 | 12 => self.erase_cells(grid, self.cursor_row, 0, cols - 1),
                    _ => {}
                }
                *printed = true;
            }
            b'L' => {
                for _ in 0..n.min(rows) {
                    grid.scroll_region_down(self.cursor_row, self.scroll_bottom);
                }
                self.cursor_col = 0;
                *printed = true;
            }
            b'M' => {
                for _ in 0..n.min(rows) {
                    grid.scroll_region_up(self.cursor_row, self.scroll_bottom);
                }
                self.cursor_col = 0;
                *printed = true;
            }
            b'@' | b'P' | b'X' => {
                if let Some(line) = grid.row_mut(self.cursor_row) {
                    let len = line.cells.len();
                    let col = self.cursor_col.min(len.saturating_sub(1));
                    let count = n.min(len - col);
                    match final_byte {
                        b'@' => {
                            for _ in 0..count {
                                line.cells.insert(col, Cell::blank());
                            }
                            line.cells.truncate(len);
                        }
                        b'P' => {
                            line.cells.drain(col..col + count);
                            line.cells.extend(std::iter::repeat_n(Cell::blank(), count));
                        }
                        _ => {
                            for cell in &mut line.cells[col..col + count] {
                                *cell = Cell::blank();
                            }
                        }
                    }
                }
                *printed = true;
            }
            b'S' => {
                for _ in 0..n.min(rows) {
                    grid.scroll_region_up(self.scroll_top, self.scroll_bottom);
                }
                *printed = true;
            }
            b'T' => {
                for _ in 0..n.min(rows) {
                    grid.scroll_region_down(self.scroll_top, self.scroll_bottom);
                }
                *printed = true;
            }
            b'm' => {
                let slots = self.param_slots();
                self.apply_sgr(&slots);
            }
            b'r' if self.private.is_empty() => {
                let top = usize::try_from(p0.saturating_sub(1)).unwrap_or(0);
                let p1 = params.get(1).copied().unwrap_or(0);
                let bottom = if p1 == 0 {
                    rows - 1
                } else {
                    usize::try_from(p1 - 1).unwrap_or(0).min(rows - 1)
                };
                if top < bottom {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                } else {
                    self.scroll_top = 0;
                    self.scroll_bottom = rows - 1;
                }
                self.region_follows_bottom = false;
                self.cursor_row = 0;
                self.cursor_col = 0;
                self.wrap_pending = false;
            }
            b's' => self.saved = Some(self.snapshot()),
            b'u' => self.restore_saved(),
            b'n' => match p0 {
                5 => events.push(ParserEvent::Respond(b"\x1b[0n".to_vec())),
                6 => {
                    let r = self.cursor_row + 1;
                    let c = self.cursor_col + 1;
                    events.push(ParserEvent::Respond(format!("\x1b[{r};{c}R").into_bytes()));
                }
                _ => {}
            },
            b'c' if self.private.is_empty() => {
                events.push(ParserEvent::Respond(b"\x1b[?6c".to_vec()));
            }
            b'h' | b'l' => self.apply_modes(&params, final_byte == b'h', events),
            _ => {}
        }
    }

    fn apply_modes(&mut self, params: &[u32], set: bool, events: &mut Vec<ParserEvent>) {
        if !self.private.is_empty() && self.private != b"?" {
            return;
        }
        for &m in params {
            match m {
                1 => self.application_cursor_keys = set,
                7 => self.auto_wrap = set,
                25 => {
                    self.cursor_visible = set;
                    events.push(ParserEvent::CursorVisibility(set));
                }
                47 | 1047 => {
                    if set {
                        events.push(ParserEvent::EnterAltScreen);
                        self.alt_home();
                    } else {
                        events.push(ParserEvent::ExitAltScreen);
                    }
                }
                1049 => {
                    if set {
                        self.saved = Some(self.snapshot());
                        events.push(ParserEvent::EnterAltScreen);
                        self.alt_home();
                    } else {
                        events.push(ParserEvent::ExitAltScreen);
                        self.restore_saved();
                    }
                }
                1048 => {
                    if set {
                        self.saved = Some(self.snapshot());
                    } else {
                        self.restore_saved();
                    }
                }
                1000 | 1002 | 1003 => {
                    if m == 1000 {
                        self.mouse_protocol = if set {
                            MouseProtocol::Click
                        } else {
                            MouseProtocol::Off
                        };
                    } else if m == 1002 {
                        self.mouse_protocol = if set {
                            MouseProtocol::Drag
                        } else {
                            self.mouse_protocol
                        };
                    } else {
                        self.mouse_protocol = if set {
                            MouseProtocol::Any
                        } else {
                            self.mouse_protocol
                        };
                    }
                }
                1006 => self.mouse_sgr_coordinates = set,
                1015 => self.mouse_urxvt_coordinates = set,
                2004 => self.bracketed_paste = set,
                _ => {}
            }
        }
    }

    fn apply_sgr(&mut self, params: &[(u32, bool)]) {
        let mut i = 0;
        while i < params.len() {
            let (p, colon) = params[i];
            if p == 38 || p == 48 {
                let (color, extra) = match params.get(i + 1) {
                    Some((5, _)) => (
                        params
                            .get(i + 2)
                            .map(|&(idx, _)| ansi256(u8::try_from(idx).unwrap_or(0))),
                        3,
                    ),
                    Some((2, _)) => {
                        // Issue #4: only the colon form (`38:2::r:g:b`)
                        // carries a colour-id slot between the 2 and the
                        // red component; `38;2;0;255;0` is plain RGB and
                        // its leading zero must not be swallowed.
                        let j = if colon { i + 3 } else { i + 2 };
                        let rgb = match (params.get(j), params.get(j + 1), params.get(j + 2)) {
                            (Some(&(r, _)), Some(&(g, _)), Some(&(b, _))) => Some(Color::rgb(
                                u8::try_from(r).unwrap_or(0),
                                u8::try_from(g).unwrap_or(0),
                                u8::try_from(b).unwrap_or(0),
                            )),
                            _ => None,
                        };
                        (rgb, j + 3 - i)
                    }
                    _ => (None, 2),
                };
                if let Some(c) = color {
                    if p == 38 {
                        self.pen_fg = Some(c);
                    } else {
                        self.pen_bg = Some(c);
                    }
                }
                i += extra;
                continue;
            }
            match p {
                0 => {
                    self.pen_fg = None;
                    self.pen_bg = None;
                    self.pen_attrs = CellAttribute::empty();
                    // RST closes an open OSC 8 link (issue #41).
                    self.pen_link = 0;
                }
                1 => self.pen_attrs = self.pen_attrs | CellAttribute::BOLD,
                2 => self.pen_attrs = self.pen_attrs | CellAttribute::DIM,
                3 => self.pen_attrs = self.pen_attrs | CellAttribute::ITALIC,
                4 => self.pen_attrs = self.pen_attrs | CellAttribute::UNDERLINE,
                5 => self.pen_attrs = self.pen_attrs | CellAttribute::BLINK,
                7 => self.pen_attrs = self.pen_attrs | CellAttribute::INVERSE,
                8 => self.pen_attrs = self.pen_attrs | CellAttribute::HIDDEN,
                9 => self.pen_attrs = self.pen_attrs | CellAttribute::STRIKE,
                21 => self.pen_attrs = self.pen_attrs.without(CellAttribute::UNDERLINE),
                22 => {
                    self.pen_attrs = self
                        .pen_attrs
                        .without(CellAttribute::BOLD | CellAttribute::DIM);
                }
                23 => self.pen_attrs = self.pen_attrs.without(CellAttribute::ITALIC),
                24 => self.pen_attrs = self.pen_attrs.without(CellAttribute::UNDERLINE),
                25 => self.pen_attrs = self.pen_attrs.without(CellAttribute::BLINK),
                27 => self.pen_attrs = self.pen_attrs.without(CellAttribute::INVERSE),
                28 => self.pen_attrs = self.pen_attrs.without(CellAttribute::HIDDEN),
                29 => self.pen_attrs = self.pen_attrs.without(CellAttribute::STRIKE),
                39 => self.pen_fg = None,
                49 => self.pen_bg = None,
                30..=37 => self.pen_fg = Some(ansi_palette()[p as usize - 30]),
                40..=47 => self.pen_bg = Some(ansi_palette()[p as usize - 40]),
                90..=97 => self.pen_fg = Some(ansi_palette()[p as usize - 90 + 8]),
                100..=107 => self.pen_bg = Some(ansi_palette()[p as usize - 100 + 8]),
                _ => {}
            }
            i += 1;
        }
    }

    /// Accumulate a multibyte UTF-8 sequence and print it once complete.
    ///
    /// Bytes arrive in arbitrary chunks from ConPTY, so partial sequences
    /// stay in `utf8_pending` across `advance` calls; invalid bytes become
    /// U+FFFD. The expected-length check is what makes 4-byte sequences
    /// (emoji) survive a split across two `advance` calls.
    fn utf8_byte(&mut self, grid: &mut Grid, b: u8, printed: &mut bool) {
        self.utf8_pending.push(b);
        loop {
            let first = self.utf8_pending[0];
            let expected = match first {
                0xC0..=0xDF => 2,
                0xE0..=0xEF => 3,
                0xF0..=0xF7 => 4,
                // A lone continuation byte: not the start of anything.
                _ => {
                    self.utf8_pending.clear();
                    self.print_char(grid, '\u{FFFD}');
                    *printed = true;
                    return;
                }
            };
            if self.utf8_pending.len() < expected {
                // Keep buffering: the sequence is incomplete, not invalid.
                return;
            }
            let buf = std::mem::take(&mut self.utf8_pending);
            match std::str::from_utf8(&buf) {
                Ok(text) => {
                    for ch in text.chars() {
                        self.print_char(grid, ch);
                        *printed = true;
                    }
                }
                Err(_) => self.print_char(grid, '\u{FFFD}'),
            }
            *printed = true;
            if self.utf8_pending.is_empty() {
                return;
            }
        }
    }
}

/// Color for an ANSI-256 index (16 palette + 6x6x6 cube + 24 grays).
#[must_use]
pub fn ansi256(n: u8) -> Color {
    const PALETTE: [Color; 16] = ansi_palette();
    const fn component(v: u8) -> u8 {
        if v == 0 { 0 } else { 55 + 40 * v }
    }
    match n {
        0..=15 => PALETTE[usize::from(n)],
        16..=231 => {
            let v = n - 16;
            Color::rgb(component(v / 36), component((v / 6) % 6), component(v % 6))
        }
        _ => {
            let g = 8 + 10 * (n - 232);
            Color::rgb(g, g, g)
        }
    }
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

/// Encode one mouse report for the pane's active protocol.
#[must_use]
pub fn mouse_report_bytes(
    button: u16,
    col: usize,
    row: usize,
    release: bool,
    sgr: bool,
    urxvt: bool,
) -> Vec<u8> {
    let col = col.saturating_add(1);
    let row = row.saturating_add(1);
    if sgr {
        format!("[{button};{col};{row}{}", if release { 'm' } else { 'M' }).into_bytes()
    } else if urxvt {
        format!("[{};{col};{row}M", button.saturating_add(32)).into_bytes()
    } else {
        vec![
            0x1b,
            b'[',
            b'M',
            32u8.saturating_add(u8::try_from(button).unwrap_or(0)),
            33u8.saturating_add(u8::try_from(col).unwrap_or(255)),
            33u8.saturating_add(u8::try_from(row).unwrap_or(255)),
        ]
    }
}

/// Wrap paste text for the shell, honoring bracketed-paste mode.
///
/// GHSA-mjq7-4ghj-vr28: clipboard text can carry escape sequences. Embedded
/// ESC and C0 control bytes are stripped (newlines, CR and tab are kept),
/// so a paste can never inject cursor addressing, title changes, mouse
/// mode toggles or a PTY write back into the terminal.
#[must_use]
pub fn bracketed_paste_bytes(text: &str, enabled: bool) -> Vec<u8> {
    let safe: String = text
        .chars()
        .filter(|c| !c.is_ascii_control() || matches!(c, '\n' | '\r' | '\t'))
        .collect();
    if enabled {
        format!("\x1b[200~{safe}\x1b[201~").into_bytes()
    } else {
        safe.as_bytes().to_vec()
    }
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod parser_tests;
