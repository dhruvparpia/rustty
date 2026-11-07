//! The startup banner: the crab, status, key commands and a tip.
//!
//! The banner is ordinary terminal text fed to a pane before its shell starts,
//! so it scrolls, selects and searches like any other output. Pure: this module
//! builds the text, and the window decides when to show it.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use crate::color::Color;
use crate::grid::Cell;
use crate::pane::Pane;
use crate::theme::Theme;

/// Which pose the crab's claws are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// Claws open.
    Open,
    /// Claws snapped shut.
    Closed,
}

/// Crab width in cells.
pub const CRAB_COLS: usize = 26;
/// Crab height in cells.
pub const CRAB_ROWS: usize = 8;
/// Narrowest window that fits the crab beside the text.
pub const FULL_MIN_COLS: usize = 78;
/// Narrowest window that fits the banner at all.
pub const TEXT_MIN_COLS: usize = 52;
/// Shortest window that gets a banner.
pub const MIN_ROWS: usize = 14;

/// Text colors, all from the theme so light themes work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colors {
    /// `rustty`, commands, `Tip:`.
    pub accent: Color,
    /// Version and status.
    pub muted: Color,
    /// Descriptions, tip text and the crab's `>_`.
    pub foreground: Color,
    /// Background behind commands.
    pub raised: Color,
}

impl Colors {
    /// The banner colors for `theme`.
    #[must_use]
    pub fn from_theme(theme: &Theme) -> Self {
        let ui = theme.ui_colors();
        Self {
            accent: ui.accent,
            muted: ui.muted,
            foreground: theme.foreground,
            raised: theme.background.mix(theme.foreground, 0.07),
        }
    }
}

/// Everything the banner shows.
#[derive(Debug, Clone, Copy)]
pub struct Banner<'a> {
    /// Version text, such as `0.6.0`.
    pub version: &'a str,
    /// Pane width in cells.
    pub cols: usize,
    /// Pane height in cells.
    pub rows: usize,
    /// The command name to show: see [`exe_for_program`].
    pub exe: &'a str,
    /// Tip text, without the `Tip:` label.
    pub tip: &'a str,
    /// Colors.
    pub colors: Colors,
    /// The crab's starting pose.
    pub frame: Frame,
}

impl Banner<'_> {
    /// The banner as terminal text, ending at the start of a fresh line.
    /// `None` when it does not fit.
    #[must_use]
    pub fn render(&self) -> Option<String> {
        if self.cols < TEXT_MIN_COLS || self.rows < MIN_ROWS {
            return None;
        }
        let crab = self
            .has_crab()
            .then(|| crab_cells(self.frame, self.colors.foreground));
        let mut out = String::with_capacity(4096);
        let text_rows = self.text_rows();
        let rows = if crab.is_some() {
            CRAB_ROWS
        } else {
            text_rows.len()
        };
        for r in 0..rows {
            if let Some(cells) = &crab {
                push_cells(&mut out, &cells[r]);
            }
            if let Some(text) = text_rows.get(r).filter(|text| !text.is_empty()) {
                out.push_str(&format!("\x1b[{}G", self.text_column() + 1));
                out.push_str(text);
            }
            out.push_str(RESET);
            out.push_str("\r\n");
        }
        out.push_str("\r\n");
        if TIP_LABEL.len() + self.tip.chars().count() <= self.cols {
            push_fg(&mut out, self.colors.accent);
            out.push_str(BOLD);
            out.push_str(TIP_LABEL);
            out.push_str(RESET);
            push_fg(&mut out, self.colors.foreground);
            out.push_str(self.tip);
            out.push_str(RESET);
            out.push_str("\r\n\r\n");
        }
        Some(out)
    }

    /// Where status text can go on the first row: just after the version.
    #[must_use]
    pub const fn status_column(&self) -> usize {
        self.text_column() + "rustty ".len() + self.version.len()
    }

    /// Whether the window is wide enough for the crab.
    #[must_use]
    pub const fn has_crab(&self) -> bool {
        self.cols >= FULL_MIN_COLS
    }

    /// First column of the text: beside the crab, or the left edge without it.
    const fn text_column(&self) -> usize {
        if self.has_crab() { CRAB_COLS + 2 } else { 0 }
    }

    /// The text for each row, already colored.
    fn text_rows(&self) -> Vec<String> {
        let c = self.colors;
        let commands = [
            ("Ctrl+Shift+P".to_string(), "Search every command"),
            ("Ctrl+Shift+Enter".to_string(), "Split this pane"),
            (
                format!("{} --list-sessions", self.exe),
                "List running sessions",
            ),
            (
                format!("{} --attach <n>", self.exe),
                "Reconnect to a session",
            ),
            ("welcome = false".to_string(), "Hide this banner"),
        ];
        let width = commands.iter().map(|(cmd, _)| cmd.len()).max().unwrap_or(0) + 2;
        let mut status = String::new();
        push_fg(&mut status, c.accent);
        status.push_str(BOLD);
        status.push_str("rustty");
        status.push_str(RESET);
        push_fg(&mut status, c.muted);
        status.push(' ');
        status.push_str(self.version);
        let mut rows = vec![status, String::new()];
        for (cmd, about) in &commands {
            let mut row = String::new();
            push_fg(&mut row, c.accent);
            push_bg(&mut row, c.raised);
            row.push_str(cmd);
            row.push_str(RESET);
            row.push_str(&" ".repeat(width - cmd.len()));
            push_fg(&mut row, c.foreground);
            row.push_str(about);
            rows.push(row);
        }
        rows
    }
}

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const TIP_LABEL: &str = "Tip: ";

fn push_fg(out: &mut String, c: Color) {
    out.push_str(&format!("\x1b[38;2;{};{};{}m", c.r, c.g, c.b));
}

fn push_bg(out: &mut String, c: Color) {
    out.push_str(&format!("\x1b[48;2;{};{};{}m", c.r, c.g, c.b));
}

/// Write one row of crab cells as colored text.
fn push_cells(out: &mut String, cells: &[Cell; CRAB_COLS]) {
    for cell in cells {
        out.push_str(RESET);
        if let Some(fg) = cell.fg {
            push_fg(out, fg);
        }
        if let Some(bg) = cell.bg {
            push_bg(out, bg);
        }
        out.push(cell.char);
    }
    out.push_str(RESET);
}

/// Upper shell and claws.
const COPPER_LIGHT: Color = Color::from_hex(0xE58A4E);
/// Lower shell and claws.
const COPPER_DARK: Color = Color::from_hex(0xD2642D);

/// The left claw over pixel rows 0-5; the right claw mirrors it.
const CLAW_OPEN: [&str; 6] = [
    "...lll.", ".lll...", "lll...l", "lll.lll", "dddddl.", "dddd...",
];
const CLAW_CLOSED: [&str; 6] = [
    "..ll...", ".lllll.", "lll.lll", "lll.lll", "dddddl.", "dddd...",
];
/// The body, shared by both poses. Rows 0-4 are blank.
const BODY: [&str; 16] = [
    "..........................",
    "..........................",
    "..........................",
    "..........................",
    "..........................",
    ".......llllllllllll.......",
    ".ddd..ll..........ll..ddd.",
    "..ddlll............llldd..",
    "...lll..c...........lll...",
    "..llll..ccc.........llll..",
    "..dddd....cc........dddd..",
    "..dddd..cc..........dddd..",
    "..dddd..c....ccccc..dddd..",
    "...ddd..............ddd...",
    "....dddddddddddddddddd....",
    ".....dddddddddddddddd.....",
];

/// The pixel at (`row`, `col`) as a frame character.
fn pixel(frame: Frame, row: usize, col: usize) -> u8 {
    let claw = match frame {
        Frame::Open => &CLAW_OPEN,
        Frame::Closed => &CLAW_CLOSED,
    };
    if let Some(line) = claw.get(row) {
        let line = line.as_bytes();
        // Claw columns win at both edges of the body.
        if let Some(&ch) = line.get(col) {
            return ch;
        }
        if let Some(&ch) = line.get(CRAB_COLS - 1 - col) {
            return ch;
        }
    }
    BODY[row].as_bytes()[col]
}

/// One cell from its top and bottom pixels.
fn half_cell(top: Option<Color>, bottom: Option<Color>) -> Cell {
    let (ch, fg, bg) = match (top, bottom) {
        (None, None) => return Cell::blank(),
        (Some(t), None) => ('\u{2580}', t, None),
        (None, Some(b)) => ('\u{2584}', b, None),
        (Some(t), Some(b)) if t == b => ('\u{2588}', t, None),
        (Some(t), Some(b)) => ('\u{2580}', t, Some(b)),
    };
    Cell {
        char: ch,
        fg: Some(fg),
        bg,
        ..Cell::blank()
    }
}

/// The status text that follows the version on the first row.
#[must_use]
pub fn status_text(sessions: usize) -> String {
    let noun = if sessions == 1 { "session" } else { "sessions" };
    format!(" \u{b7} session host running \u{b7} {sessions} {noun}")
}

/// The claws' poses and how long each is held, in milliseconds. The last pose
/// stays for good.
const CLACK_SEQUENCE: [(Frame, Option<u64>); 6] = [
    (Frame::Open, Some(420)),
    (Frame::Closed, Some(130)),
    (Frame::Open, Some(170)),
    (Frame::Closed, Some(130)),
    (Frame::Open, Some(170)),
    (Frame::Closed, None),
];

/// Cell rows of the crab that differ between poses.
const CLAW_ROWS: usize = 3;

/// The running animation: it rewrites the crab's claw cells over about a
/// second, and stops for good as soon as those cells are no longer what it
/// wrote, so it can never paint over other text.
#[derive(Debug)]
pub struct Clack {
    anchor_abs: usize,
    c_color: Color,
    animate: bool,
    has_crab: bool,
    shown: Frame,
    next: usize,
    status: Option<Receiver<Option<usize>>>,
    status_at: (usize, usize),
    colors: Colors,
}

impl Clack {
    /// Start after a banner rendered at absolute line `anchor_abs`. It rendered
    /// `Open` when `animate` is set and `Closed` otherwise. `status` delivers
    /// the session count for the text at (`status_at`: absolute line, column).
    #[must_use]
    pub fn new(
        anchor_abs: usize,
        c_color: Color,
        animate: bool,
        status: Option<Receiver<Option<usize>>>,
        status_at: (usize, usize),
        colors: Colors,
    ) -> Self {
        Self {
            anchor_abs,
            c_color,
            animate,
            has_crab: true,
            shown: if animate { Frame::Open } else { Frame::Closed },
            next: 0,
            status,
            status_at,
            colors,
        }
    }

    /// For a banner too narrow for the crab: no claw cells to check or draw,
    /// but the status text still arrives.
    #[must_use]
    pub const fn without_crab(mut self) -> Self {
        self.has_crab = false;
        self
    }

    /// Advance one step. Returns how long until the next step, or `None` when
    /// the animation is over.
    pub fn step(&mut self, pane: &mut Pane) -> Option<Duration> {
        if pane.in_alt() || pane.input_seen() || (self.has_crab && !self.crab_intact(pane)) {
            return None;
        }
        self.poll_status(pane);
        let &(frame, hold) = CLACK_SEQUENCE.get(self.next)?;
        self.next += 1;
        if self.has_crab && self.animate && frame != self.shown {
            self.write_frame(pane, frame);
        }
        hold.map(Duration::from_millis)
    }

    /// Whether the claw cells are still exactly what was last written.
    fn crab_intact(&self, pane: &Pane) -> bool {
        let want = crab_cells(self.shown, self.c_color);
        want.iter().take(CLAW_ROWS).enumerate().all(|(r, row)| {
            pane.grid()
                .line_abs(self.anchor_abs + r)
                .and_then(|line| line.cells.get(..CRAB_COLS))
                .is_some_and(|cells| cells == row.as_slice())
        })
    }

    fn write_frame(&mut self, pane: &mut Pane, frame: Frame) {
        let want = crab_cells(frame, self.c_color);
        for (r, row) in want.iter().take(CLAW_ROWS).enumerate() {
            for (c, cell) in row.iter().enumerate() {
                if let Some(dest) = pane.grid_mut().cell_abs_mut(self.anchor_abs + r, c) {
                    dest.clone_from(cell);
                }
            }
        }
        self.shown = frame;
        pane.mark_dirty();
    }

    fn poll_status(&mut self, pane: &mut Pane) {
        let Some(status) = &self.status else {
            return;
        };
        match status.try_recv() {
            Ok(Some(sessions)) => {
                self.status = None;
                self.write_status(pane, &status_text(sessions));
            }
            Ok(None) | Err(TryRecvError::Disconnected) => self.status = None,
            Err(TryRecvError::Empty) => {}
        }
    }

    /// Write the status text if those cells are still blank.
    fn write_status(&self, pane: &mut Pane, text: &str) {
        let (abs, col) = self.status_at;
        let count = text.chars().count();
        let free = pane
            .grid()
            .line_abs(abs)
            .and_then(|line| line.cells.get(col..col + count))
            .is_some_and(|cells| cells.iter().all(|cell| *cell == Cell::blank()));
        if !free {
            return;
        }
        for (i, ch) in text.chars().enumerate() {
            if let Some(dest) = pane.grid_mut().cell_abs_mut(abs, col + i) {
                *dest = Cell {
                    char: ch,
                    fg: Some(self.colors.muted),
                    ..Cell::blank()
                };
            }
        }
        pane.mark_dirty();
    }
}

/// The crab's cells for `frame`, with the `>_` in `c_color`.
#[must_use]
pub fn crab_cells(frame: Frame, c_color: Color) -> [[Cell; CRAB_COLS]; CRAB_ROWS] {
    let color = |ch: u8| match ch {
        b'l' => Some(COPPER_LIGHT),
        b'd' => Some(COPPER_DARK),
        b'c' => Some(c_color),
        _ => None,
    };
    std::array::from_fn(|row| {
        std::array::from_fn(|col| {
            half_cell(
                color(pixel(frame, row * 2, col)),
                color(pixel(frame, row * 2 + 1, col)),
            )
        })
    })
}

/// The command name that works in `program`: Linux shells under WSL need the
/// `.exe`.
#[must_use]
pub fn exe_for_program(program: &str) -> &'static str {
    let name = program.rsplit(['\\', '/']).next().unwrap_or(program);
    if name.eq_ignore_ascii_case("wsl") || name.eq_ignore_ascii_case("wsl.exe") {
        "rustty.exe"
    } else {
        "rustty"
    }
}

/// Tips: each names a real feature with its real default shortcut.
pub const TIPS: &[&str] = &[
    "Ctrl+Shift+K labels links and paths so you can open them by keyboard.",
    "Ctrl+F searches everything on screen and in your scrollback.",
    "Ctrl+Shift+B bookmarks a line so you can jump back to it later.",
    "Ctrl+Shift+Enter splits right, and Ctrl+Shift+- splits below.",
    "Tap Alt to open the window menu.",
    "Ctrl+Shift+S shows or hides the sessions sidebar.",
    "F11 toggles fullscreen.",
    "Ctrl+Plus and Ctrl+Minus change the font size, and Ctrl+0 resets it.",
    "rustty --new-session starts a shell that outlives this window.",
    "Want a new look? Press Ctrl+Shift+P and type Switch Theme.",
];

/// The tip for `seed`.
#[must_use]
pub fn pick_tip(seed: u64) -> &'static str {
    TIPS[(seed % TIPS.len() as u64) as usize]
}

/// How rustty was started, for deciding whether to greet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Launch {
    /// This is the quick terminal.
    pub quick: bool,
    /// Started with `--workspace`.
    pub workspace: bool,
    /// Started with `--attach`.
    pub attach: bool,
    /// The pane views a host session.
    pub view: bool,
    /// The `welcome` config key.
    pub welcome_enabled: bool,
    /// No other rustty window is open.
    pub first_instance: bool,
}

/// Whether this launch shows the banner.
#[must_use]
pub const fn should_show(launch: &Launch) -> bool {
    launch.welcome_enabled
        && launch.first_instance
        && !launch.quick
        && !launch.workspace
        && !launch.attach
        && !launch.view
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const TIP: &str = "Ctrl+Shift+K labels links and paths so you can open them by keyboard.";

    fn colors() -> Colors {
        Colors::from_theme(&Theme::copper_dark())
    }

    fn banner(cols: usize, rows: usize) -> Banner<'static> {
        Banner {
            version: "0.6.0",
            cols,
            rows,
            exe: "rustty",
            tip: TIP,
            colors: colors(),
            frame: Frame::Closed,
        }
    }

    fn fed(b: &Banner<'_>) -> Pane {
        let mut pane = Pane::with_size(b.cols, b.rows, 100);
        pane.feed(b.render().expect("fits").as_bytes());
        pane
    }

    fn row(pane: &Pane, r: usize) -> String {
        pane.grid().line_text(r).unwrap_or_default()
    }

    /// Text from column `col` to the end of the row.
    fn from(pane: &Pane, r: usize, col: usize) -> String {
        row(pane, r).chars().skip(col).collect()
    }

    #[test]
    fn crab_half_blocks_match_the_frames() {
        let prompt = Color::rgb(1, 2, 3);
        let cells = crab_cells(Frame::Open, prompt);
        // Empty over empty stays a plain blank.
        assert_eq!(cells[0][0].char, ' ');
        // Row 0 of Open: pixel rows 0 and 1. Column 3 has l over l.
        assert_eq!(cells[0][3].char, '█');
        assert_eq!(cells[0][3].fg, Some(COPPER_LIGHT));
        assert_eq!(cells[0][3].bg, None);
        // Column 5 has l over nothing.
        assert_eq!(cells[0][5].char, '▀');
        assert_eq!(cells[0][5].fg, Some(COPPER_LIGHT));
        // Column 1 has nothing over l.
        assert_eq!(cells[0][1].char, '▄');
        assert_eq!(cells[0][1].fg, Some(COPPER_LIGHT));
        // Row 2: pixel rows 4 and 5, d over d in the claw.
        assert_eq!(cells[2][0].char, '█');
        assert_eq!(cells[2][0].fg, Some(COPPER_DARK));
        // Row 2, column 5: l over nothing on the left claw's root.
        assert_eq!(cells[2][5].fg, Some(COPPER_LIGHT));
        // Row 4: pixel rows 8 and 9, l over l, then c over c in the prompt.
        assert_eq!(cells[4][8].char, '█');
        assert_eq!(cells[4][8].fg, Some(prompt));
    }

    #[test]
    fn half_cell_covers_every_pixel_pair() {
        let (a, b) = (Color::rgb(1, 1, 1), Color::rgb(2, 2, 2));
        assert_eq!(half_cell(None, None), Cell::blank());
        let top = half_cell(Some(a), None);
        assert_eq!((top.char, top.fg, top.bg), ('▀', Some(a), None));
        let bottom = half_cell(None, Some(a));
        assert_eq!((bottom.char, bottom.fg, bottom.bg), ('▄', Some(a), None));
        let full = half_cell(Some(a), Some(a));
        assert_eq!((full.char, full.fg, full.bg), ('█', Some(a), None));
        let both = half_cell(Some(a), Some(b));
        assert_eq!((both.char, both.fg, both.bg), ('▀', Some(a), Some(b)));
    }

    #[test]
    fn claws_mirror_each_other() {
        let cells = crab_cells(Frame::Closed, Color::rgb(1, 2, 3));
        for (r, row) in cells.iter().take(3).enumerate() {
            for c in 0..7 {
                assert_eq!(row[c].char, row[CRAB_COLS - 1 - c].char, "{r},{c}");
            }
        }
    }

    #[test]
    fn open_and_closed_differ_only_in_the_top_three_cell_rows() {
        // Break caught: animation rewriting cells it should not touch.
        let prompt = Color::rgb(1, 2, 3);
        let open = crab_cells(Frame::Open, prompt);
        let closed = crab_cells(Frame::Closed, prompt);
        assert_ne!(open[0], closed[0]);
        assert_eq!(open[3..], closed[3..]);
    }

    #[test]
    fn prompt_uses_the_given_colour() {
        // Break caught: a cream >_ that vanishes on a light theme.
        let prompt = Color::rgb(9, 8, 7);
        let cells = crab_cells(Frame::Open, prompt);
        let found = cells
            .iter()
            .flatten()
            .filter(|c| c.fg == Some(prompt) || c.bg == Some(prompt))
            .count();
        assert!(found >= 8, "{found} prompt cells");
    }

    #[test]
    fn full_banner_at_78_cols() {
        let b = banner(78, 24);
        let pane = fed(&b);
        assert!(
            from(&pane, 0, 28).starts_with("rustty 0.6.0"),
            "{}",
            row(&pane, 0)
        );
        assert!(from(&pane, 2, 28).starts_with("Ctrl+Shift+P"));
        assert!(from(&pane, 2, 52).starts_with("Search every command"));
        assert!(from(&pane, 3, 28).starts_with("Ctrl+Shift+Enter"));
        assert!(from(&pane, 4, 28).starts_with("rustty --list-sessions"));
        assert!(from(&pane, 4, 52).starts_with("List running sessions"));
        assert!(from(&pane, 5, 28).starts_with("rustty --attach <n>"));
        assert!(from(&pane, 6, 28).starts_with("welcome = false"));
        assert!(from(&pane, 6, 52).starts_with("Hide this banner"));
        assert_eq!(row(&pane, 8), "");
        assert_eq!(row(&pane, 9), format!("Tip: {TIP}"));
        assert_eq!(pane.parser().cursor_row, 11);
        assert_eq!(pane.parser().cursor_col, 0);
    }

    #[test]
    fn the_crab_lands_in_the_grid_exactly() {
        // Break caught: colors or characters lost between text and cells.
        let pane = fed(&banner(78, 24));
        let want = crab_cells(Frame::Closed, colors().foreground);
        for (r, cells) in want.iter().enumerate() {
            let got = pane.grid().row(r).expect("row");
            for (c, cell) in cells.iter().enumerate() {
                assert_eq!(&got.cells[c], cell, "{r},{c}");
            }
        }
    }

    #[test]
    fn banner_text_uses_its_colours() {
        let pane = fed(&banner(78, 24));
        let c = colors();
        let cells = &pane.grid().row(0).expect("row").cells;
        assert_eq!(cells[28].fg, Some(c.accent), "rustty");
        assert_eq!(cells[35].fg, Some(c.muted), "version");
        let cells = &pane.grid().row(2).expect("row").cells;
        assert_eq!(cells[28].fg, Some(c.accent), "command");
        assert_eq!(cells[28].bg, Some(c.raised), "command background");
        assert_eq!(
            cells[51].bg, None,
            "the raised background stops at the text"
        );
        assert_eq!(cells[52].fg, Some(c.foreground), "description");
    }

    #[test]
    fn crab_drops_at_77_cols() {
        let pane = fed(&banner(77, 24));
        assert!(row(&pane, 0).starts_with("rustty 0.6.0"));
        assert!(from(&pane, 2, 0).starts_with("Ctrl+Shift+P"));
        assert!(from(&pane, 2, 24).starts_with("Search every command"));
        assert!(!row(&pane, 1).contains('█'));
        assert_eq!(row(&pane, 8), format!("Tip: {TIP}"));
        assert_eq!(pane.parser().cursor_row, 10);
    }

    #[test]
    fn banner_drops_below_52_cols_or_14_rows() {
        assert!(banner(51, 24).render().is_none());
        assert!(banner(80, 13).render().is_none());
        assert!(banner(52, 14).render().is_some());
    }

    #[test]
    fn wsl_uses_the_exe_name_and_still_fits() {
        let b = Banner {
            exe: "rustty.exe",
            ..banner(78, 24)
        };
        let pane = fed(&b);
        assert!(from(&pane, 4, 28).starts_with("rustty.exe --list-sessions"));
        assert!(from(&pane, 4, 56).starts_with("List running sessions"));
        for r in 0..8 {
            assert!(row(&pane, r).chars().count() <= 78, "row {r} wrapped");
        }
        assert_eq!(pane.parser().cursor_row, 11, "nothing wrapped");
    }

    #[test]
    fn exe_name_follows_the_shell() {
        assert_eq!(exe_for_program("wsl.exe"), "rustty.exe");
        assert_eq!(exe_for_program("wsl"), "rustty.exe");
        assert_eq!(
            exe_for_program(r"C:\Windows\System32\WSL.EXE"),
            "rustty.exe"
        );
        assert_eq!(exe_for_program("powershell.exe"), "rustty");
        assert_eq!(
            exe_for_program(r"C:\Program Files\Git\bin\bash.exe"),
            "rustty"
        );
    }

    #[test]
    fn every_tip_fits_on_one_line() {
        assert!(TIPS.len() >= 6);
        for tip in TIPS {
            assert!(
                tip.chars().count() + "Tip: ".len() <= FULL_MIN_COLS,
                "{tip}"
            );
        }
    }

    #[test]
    fn pick_tip_covers_every_tip() {
        for (i, tip) in TIPS.iter().enumerate() {
            assert_eq!(pick_tip(i as u64), *tip);
        }
        assert_eq!(pick_tip(TIPS.len() as u64), TIPS[0]);
    }

    #[test]
    fn tip_is_left_out_when_it_would_wrap() {
        let pane = fed(&banner(60, 24));
        for r in 0..24 {
            assert!(!row(&pane, r).contains("Tip:"), "row {r}");
        }
        assert_eq!(pane.parser().cursor_row, 8);
    }

    #[test]
    fn banner_leaves_the_pen_reset() {
        // Break caught: the shell's first output inheriting banner colors.
        let pane = fed(&banner(78, 24));
        assert_eq!(pane.parser().pen_fg, None);
        assert_eq!(pane.parser().pen_bg, None);
        assert_eq!(pane.parser().pen_attrs, crate::grid::CellAttribute::empty());
    }

    #[test]
    fn banner_emits_only_color_and_column_sequences() {
        // Break caught: a query whose reply would land in the shell's input.
        let text = banner(78, 24).render().expect("fits");
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == 0x1b {
                assert_eq!(bytes.get(i + 1), Some(&b'['), "only CSI");
                let mut j = i + 2;
                while !bytes[j].is_ascii_alphabetic() {
                    j += 1;
                }
                assert!(
                    matches!(bytes[j], b'm' | b'G'),
                    "final byte {}",
                    bytes[j] as char
                );
                i = j;
            }
            i += 1;
        }
    }

    /// WCAG contrast ratio.
    fn contrast(a: Color, b: Color) -> f64 {
        let luminance = |c: Color| {
            let linear = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn light_theme_stays_readable() {
        // Break caught: banner text that disappears on a light Windows theme.
        for theme in [Theme::copper_light(), Theme::copper_dark()] {
            let c = Colors::from_theme(&theme);
            assert!(
                contrast(c.foreground, theme.background) >= 4.5,
                "{}",
                theme.name
            );
            assert!(contrast(c.muted, theme.background) >= 4.5, "{}", theme.name);
            assert!(
                contrast(c.accent, theme.background) >= 3.0,
                "{}",
                theme.name
            );
            assert!(contrast(c.accent, c.raised) >= 3.0, "{}", theme.name);
            assert_ne!(c.raised, theme.background, "{}", theme.name);
        }
    }

    #[test]
    fn colours_come_from_the_theme() {
        let theme = Theme::copper_dark();
        let ui = theme.ui_colors();
        let c = Colors::from_theme(&theme);
        assert_eq!(c.accent, ui.accent);
        assert_eq!(c.muted, ui.muted);
        assert_eq!(c.foreground, theme.foreground);
    }

    #[test]
    fn appearance_rules() {
        let plain = Launch {
            quick: false,
            workspace: false,
            attach: false,
            view: false,
            welcome_enabled: true,
            first_instance: true,
        };
        assert!(should_show(&plain));
        for (name, launch) in [
            (
                "quick",
                Launch {
                    quick: true,
                    ..plain
                },
            ),
            (
                "workspace",
                Launch {
                    workspace: true,
                    ..plain
                },
            ),
            (
                "attach",
                Launch {
                    attach: true,
                    ..plain
                },
            ),
            (
                "view",
                Launch {
                    view: true,
                    ..plain
                },
            ),
            (
                "disabled",
                Launch {
                    welcome_enabled: false,
                    ..plain
                },
            ),
            (
                "second window",
                Launch {
                    first_instance: false,
                    ..plain
                },
            ),
        ] {
            assert!(!should_show(&launch), "{name}");
        }
    }

    #[test]
    fn welcome_config_key_defaults_on_and_parses_false() {
        assert!(Config::parse("").expect("empty").welcome);
        assert!(!Config::parse("welcome = false\n").expect("off").welcome);
    }

    #[test]
    fn status_column_is_just_after_the_version() {
        assert_eq!(banner(78, 24).status_column(), 28 + "rustty 0.6.0".len());
        assert_eq!(banner(60, 24).status_column(), "rustty 0.6.0".len());
    }

    /// A pane showing the animated banner, and its clack.
    fn clacking(scrollback: usize) -> (Pane, Clack) {
        let b = Banner {
            frame: Frame::Open,
            ..banner(78, 24)
        };
        let mut pane = Pane::with_size(78, 24, scrollback);
        pane.feed(b.render().expect("fits").as_bytes());
        let clack = Clack::new(
            0,
            colors().foreground,
            true,
            None,
            (0, b.status_column()),
            colors(),
        );
        (pane, clack)
    }

    /// The crab's top three rows as they are in the grid now.
    fn claw_rows(pane: &Pane) -> Vec<Vec<Cell>> {
        (0..3)
            .map(|r| pane.grid().line_abs(r).expect("line").cells[..CRAB_COLS].to_vec())
            .collect()
    }

    fn claw_frame(frame: Frame) -> Vec<Vec<Cell>> {
        crab_cells(frame, colors().foreground)[..3]
            .iter()
            .map(|row| row.to_vec())
            .collect()
    }

    fn ms(n: u64) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_millis(n))
    }

    #[test]
    fn sequence_is_open_closed_open_closed_open_closed() {
        let (mut pane, mut clack) = clacking(100);
        let expected = [
            (ms(420), Frame::Open),
            (ms(130), Frame::Closed),
            (ms(170), Frame::Open),
            (ms(130), Frame::Closed),
            (ms(170), Frame::Open),
            (None, Frame::Closed),
        ];
        for (delay, frame) in expected {
            assert_eq!(clack.step(&mut pane), delay);
            assert_eq!(claw_rows(&pane), claw_frame(frame));
        }
    }

    #[test]
    fn scrolling_shell_output_does_not_stop_the_clack() {
        // Break caught: the shell's first output cancelling the animation, so
        // the claws never move.
        let (mut pane, mut clack) = clacking(100);
        for delay in [ms(420), ms(130), ms(170), ms(130), ms(170), None] {
            assert_eq!(clack.step(&mut pane), delay);
            pane.feed(b"shell output\r\nmore output\r\nand more\r\n");
        }
        assert_eq!(claw_rows(&pane), claw_frame(Frame::Closed));
    }

    #[test]
    fn overwritten_crab_stops_and_is_left_alone() {
        // Break caught: the animation painting over the user's text.
        let (mut pane, mut clack) = clacking(100);
        assert_eq!(clack.step(&mut pane), ms(420));
        pane.feed(b"\x1b[1;4HX");
        assert_eq!(clack.step(&mut pane), None);
        assert_eq!(pane.grid().line_abs(0).expect("line").cells[3].char, 'X');
    }

    #[test]
    fn burst_that_evicts_the_crab_stops_safely() {
        // Review focus: a profile script printing thousands of lines.
        let (mut pane, mut clack) = clacking(10);
        assert_eq!(clack.step(&mut pane), ms(420));
        for i in 0..10_000 {
            pane.feed(format!("line {i}\r\n").as_bytes());
        }
        let last = pane.grid().line_text(22);
        assert_eq!(clack.step(&mut pane), None);
        assert_eq!(pane.grid().line_text(22), last, "later lines intact");
    }

    #[test]
    fn cleared_screen_stops_the_clack() {
        let (mut pane, mut clack) = clacking(100);
        assert_eq!(clack.step(&mut pane), ms(420));
        pane.feed(b"\x1b[2J\x1b[H");
        assert_eq!(clack.step(&mut pane), None);
        for r in 0..3 {
            assert_eq!(pane.grid().line_text(r).as_deref(), Some(""), "row {r}");
        }
    }

    #[test]
    fn alt_screen_stops_the_clack() {
        let (mut pane, mut clack) = clacking(100);
        assert_eq!(clack.step(&mut pane), ms(420));
        pane.feed(b"\x1b[?1049h");
        assert_eq!(clack.step(&mut pane), None);
        pane.feed(b"\x1b[?1049l");
        assert_eq!(
            claw_rows(&pane),
            claw_frame(Frame::Open),
            "main screen untouched"
        );
    }

    #[test]
    fn typing_stops_the_clack() {
        let (mut pane, mut clack) = clacking(100);
        assert_eq!(clack.step(&mut pane), ms(420));
        let _ = pane.send_input(b"x");
        assert_eq!(clack.step(&mut pane), None);
    }

    #[test]
    fn without_animation_the_frame_never_changes() {
        // Break caught: motion with Windows animation effects off.
        let b = banner(78, 24);
        let mut pane = fed(&b);
        let mut clack = Clack::new(
            0,
            colors().foreground,
            false,
            None,
            (0, b.status_column()),
            colors(),
        );
        for delay in [ms(420), ms(130), ms(170), ms(130), ms(170), None] {
            assert_eq!(clack.step(&mut pane), delay);
            assert_eq!(claw_rows(&pane), claw_frame(Frame::Closed));
        }
    }

    #[test]
    fn status_text_counts_sessions() {
        assert_eq!(
            status_text(2),
            " \u{b7} session host running \u{b7} 2 sessions"
        );
        assert_eq!(
            status_text(1),
            " \u{b7} session host running \u{b7} 1 session"
        );
        assert_eq!(
            status_text(0),
            " \u{b7} session host running \u{b7} 0 sessions"
        );
    }

    fn with_status(answer: Option<usize>) -> Pane {
        let b = Banner {
            frame: Frame::Open,
            ..banner(78, 24)
        };
        let mut pane = fed(&b);
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(answer).expect("send");
        let mut clack = Clack::new(
            0,
            colors().foreground,
            true,
            Some(rx),
            (0, b.status_column()),
            colors(),
        );
        assert_eq!(clack.step(&mut pane), ms(420));
        pane
    }

    #[test]
    fn status_is_filled_in_when_it_arrives() {
        let pane = with_status(Some(2));
        let line = pane.grid().line_text_abs(0).expect("line");
        assert!(
            line.ends_with("rustty 0.6.0 \u{b7} session host running \u{b7} 2 sessions"),
            "{line}"
        );
        let cells = &pane.grid().line_abs(0).expect("line").cells;
        assert_eq!(cells[28 + 12].fg, Some(colors().muted));
    }

    #[test]
    fn no_host_leaves_the_version_alone() {
        let pane = with_status(None);
        let line = pane.grid().line_text_abs(0).expect("line");
        assert!(line.ends_with("rustty 0.6.0"), "{line}");
    }

    #[test]
    fn status_is_not_written_over_other_text() {
        let b = Banner {
            frame: Frame::Open,
            ..banner(78, 24)
        };
        let mut pane = fed(&b);
        // Something else already wrote where the status would go.
        pane.feed(b"\x1b[1;50HZ");
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Some(2)).expect("send");
        let mut clack = Clack::new(
            0,
            colors().foreground,
            true,
            Some(rx),
            (0, b.status_column()),
            colors(),
        );
        assert_eq!(clack.step(&mut pane), ms(420));
        let line = pane.grid().line_text_abs(0).expect("line");
        assert!(line.contains('Z'), "{line}");
        assert!(!line.contains("session host"), "{line}");
    }

    #[test]
    fn status_arrives_without_a_crab() {
        // Break caught: the narrow banner's text failing the claw check, which
        // ended the animation before the session count could land.
        let b = banner(60, 24);
        let mut pane = fed(&b);
        let (tx, rx) = std::sync::mpsc::channel();
        let mut clack = Clack::new(
            0,
            colors().foreground,
            true,
            Some(rx),
            (0, b.status_column()),
            colors(),
        )
        .without_crab();
        assert_eq!(clack.step(&mut pane), ms(420));
        tx.send(Some(2)).expect("send");
        assert_eq!(clack.step(&mut pane), ms(130));
        let line = pane.grid().line_text(0).expect("line");
        assert_eq!(
            line,
            "rustty 0.6.0 \u{b7} session host running \u{b7} 2 sessions"
        );
        // No claw cells were drawn over the text.
        assert_eq!(
            pane.grid().line_text(2).as_deref().map(|l| &l[..12]),
            Some("Ctrl+Shift+P")
        );
    }
}
