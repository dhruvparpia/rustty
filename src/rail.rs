//! The sessions sidebar: what each session is doing, and where its row goes.
//!
//! It reads the signals any terminal program already sends, so agent CLIs and
//! other harnesses work with no setup: a spinner in the window title, recent
//! output, a progress report, a bell or a notification. Pure, with no Win32 or
//! Direct2D dependency: the window gathers the signals and the renderer draws
//! the rows.

use std::time::Duration;

/// Sidebar width.
pub const RAIL_WIDTH: f32 = 248.0;
/// Below this client width the sidebar collapses, so the terminal keeps room.
pub const RAIL_MIN_CLIENT: f32 = 720.0;
/// Height of the "New session" row.
pub const NEW_ROW_HEIGHT: f32 = 36.0;
/// Height of a project heading.
pub const SECTION_HEIGHT: f32 = 26.0;
/// Height of a session row.
pub const ROW_HEIGHT: f32 = 48.0;
/// Width of the close target at the right of a row.
pub const CLOSE_WIDTH: f32 = 28.0;

/// How recent output has to be to count as working.
const OUTPUT_WINDOW: Duration = Duration::from_millis(1500);
/// Output this soon after a keystroke is probably just its echo.
const ECHO_WINDOW: Duration = Duration::from_millis(600);
/// Work shorter than this is not worth a "finished" mark.
const FINISHED_MIN_MS: u64 = 3000;
/// A spinner title with no output for this long was left behind by a program
/// that is gone. Spinners animate, so a live one keeps writing.
const SPINNER_STALE: Duration = Duration::from_secs(60);
/// Progress with no output for this long was left behind in the same way.
const PROGRESS_STALE: Duration = Duration::from_secs(300);
/// How many brightness steps a pulse has.
const PULSE_LEVELS: f32 = 8.0;
/// Output this soon after a resize is the program repainting.
const RESIZE_REPAINT: Duration = Duration::from_millis(1000);
/// Longest attention message kept from a notification.
const MESSAGE_MAX_CHARS: usize = 120;

/// What a session is doing, in the order it matters to the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Asked for attention: a bell or a notification.
    NeedsYou,
    /// A program reported that it failed (STATUS-001).
    Failed,
    /// A restart stopped its agent (RESTORE-003).
    Interrupted,
    /// Busy right now.
    Working,
    /// Finished while nobody was looking.
    Finished,
    /// Waiting, with nothing to report.
    Idle,
    /// Its shell has exited.
    Exited,
    /// A host session this window is not showing.
    Background,
}

/// What one pane is reporting.
#[derive(Debug, Clone, Copy, Default)]
pub struct Signals<'a> {
    /// Its window title.
    pub title: &'a str,
    /// Shell integration says a command is running.
    pub command_running: bool,
    /// The shell has exited.
    pub exited: bool,
    /// A bell or notification has not been seen yet.
    pub attention: bool,
    /// A program reported progress (`OSC 9;4`).
    pub progress: bool,
    /// Time since the last output.
    pub output_age: Option<Duration>,
    /// Time since the last input.
    pub input_age: Option<Duration>,
}

/// Whether a title starts with a spinner: agent CLIs animate one while busy.
#[must_use]
pub fn has_spinner(title: &str) -> bool {
    title.chars().next().is_some_and(|c| {
        ('\u{2801}'..='\u{28FF}').contains(&c) || ('\u{25D0}'..='\u{25D3}').contains(&c)
    })
}

/// Whether the pane is busy right now.
#[must_use]
pub fn is_working(s: &Signals<'_>) -> bool {
    if s.exited {
        return false;
    }
    let recent_output = s.output_age.is_some_and(|age| age < OUTPUT_WINDOW)
        && s.input_age.is_none_or(|age| age >= ECHO_WINDOW);
    let fresh = |limit| s.output_age.is_none_or(|age| age < limit);
    (s.progress && fresh(PROGRESS_STALE))
        || s.command_running
        || (has_spinner(s.title) && fresh(SPINNER_STALE))
        || recent_output
}

/// Whether output is the program's own work, not the repaint a resize causes.
#[must_use]
pub fn output_is_work(since_resize: Option<Duration>) -> bool {
    since_resize.is_none_or(|age| age >= RESIZE_REPAINT)
}

/// Whether opening a background session should take control of it.
#[must_use]
pub const fn opens_with_control(controllers: Option<u64>) -> bool {
    matches!(controllers, Some(0))
}

/// The tab label for a host session opened from the sidebar.
#[must_use]
pub fn view_label(alias: u32, name: Option<&str>, control: bool) -> String {
    let base = match name {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => format!("Session {alias}"),
    };
    if control {
        base
    } else {
        format!("{base} · watch")
    }
}

/// A working session's dot brightness at `secs`, in a few fixed steps so it
/// repaints only on a step and draws with a small set of colours.
#[must_use]
pub fn pulse_at(secs: f32) -> f32 {
    let wave = (secs * 3.0).sin() * 0.5 + 0.5;
    0.45 + 0.55 * (wave * PULSE_LEVELS).round() / PULSE_LEVELS
}

/// The title a row shows: no spinner, no control characters, bounded.
#[must_use]
pub fn row_title(title: &str, fallback: &str) -> String {
    let cleaned = clean_message(clean_title(title));
    if cleaned.trim().is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

/// Whether closing a tab from its row should ask first. A row showing work or
/// a question always asks: the close target sits on a row made to be clicked.
#[must_use]
pub const fn close_needs_confirm(
    status: Status,
    profile_confirms: bool,
    live: bool,
    last: bool,
) -> bool {
    matches!(status, Status::Working | Status::NeedsYou) || (profile_confirms && (live || last))
}

/// The x a press that began in the grid reports while it drags over the
/// sidebar: the grid's edge, the way any drag past a terminal's edge clamps.
#[must_use]
pub const fn drag_x(x: f32, rail_width: f32, pressed_in_grid: bool) -> f32 {
    if pressed_in_grid && x < rail_width {
        rail_width
    } else {
        x
    }
}

/// Remembers whether a session finished while nobody was looking.
#[derive(Debug, Clone, Copy, Default)]
pub struct Activity {
    working_since_ms: Option<u64>,
    finished: bool,
}

impl Activity {
    /// Forget work in progress, after a time when nothing was watching.
    pub const fn reset(&mut self) {
        self.working_since_ms = None;
        self.finished = false;
    }

    /// The status now. `viewed` means the person can see this session.
    pub fn step(&mut self, s: &Signals<'_>, viewed: bool, now_ms: u64) -> Status {
        if s.exited {
            self.working_since_ms = None;
            return Status::Exited;
        }
        let working = is_working(s);
        if working {
            self.working_since_ms.get_or_insert(now_ms);
            self.finished = false;
        } else if let Some(since) = self.working_since_ms.take() {
            self.finished = now_ms.saturating_sub(since) >= FINISHED_MIN_MS;
        }
        if viewed {
            self.finished = false;
        }
        if s.attention && !viewed {
            Status::NeedsYou
        } else if working {
            Status::Working
        } else if self.finished {
            Status::Finished
        } else {
            Status::Idle
        }
    }
}

/// The status to show for a tab, from the statuses of its panes.
#[must_use]
pub fn most_urgent(statuses: impl IntoIterator<Item = Status>) -> Status {
    let rank = |status: &Status| match status {
        Status::NeedsYou => 0,
        Status::Failed => 1,
        Status::Interrupted => 2,
        Status::Working => 3,
        Status::Finished => 4,
        Status::Idle => 5,
        Status::Background => 6,
        Status::Exited => 7,
    };
    statuses
        .into_iter()
        .min_by_key(rank)
        .unwrap_or(Status::Idle)
}

/// A title without its spinner or status mark.
#[must_use]
pub fn clean_title(title: &str) -> &str {
    let is_mark = |c: char| {
        c.is_whitespace()
            || ('\u{2800}'..='\u{28FF}').contains(&c)
            || ('\u{25D0}'..='\u{25D3}').contains(&c)
            || matches!(
                c,
                '\u{2733}' | '\u{2736}' | '\u{273B}' | '\u{273D}' | '\u{2722}' | '\u{FE0F}'
            )
    };
    title.trim_start_matches(is_mark)
}

/// The project a working directory belongs to: its last path component.
#[must_use]
pub fn project_of(cwd: Option<&str>) -> String {
    let Some(cwd) = cwd.map(str::trim).filter(|c| !c.is_empty()) else {
        return "Other".to_string();
    };
    // A worktree of ours belongs to its main checkout.
    let normal = cwd.replace('\\', "/");
    let main = normal
        .find(&format!("/{}/", crate::worktree::WORKTREE_DIR))
        .map_or(normal.as_str(), |at| &normal[..at]);
    let trimmed = main.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    // A drive or filesystem root has no folder name of its own.
    if last.is_empty() || last.ends_with(':') {
        clean_message(cwd)
    } else {
        clean_message(last)
    }
}

/// A notification's text, made safe to draw: no control characters, bounded.
#[must_use]
pub fn clean_message(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(MESSAGE_MAX_CHARS)
        .collect()
}

/// What a row opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A tab in this window.
    Tab(usize),
    /// A host session, by alias.
    Host(u32),
}

/// One session in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// What it opens.
    pub target: Target,
    /// Main text.
    pub title: String,
    /// The line under the title.
    pub detail: String,
    /// Heading it is grouped under.
    pub project: String,
    /// What it is doing.
    pub status: Status,
    /// It is the session on screen.
    pub active: bool,
}

/// The line under a row's title.
#[must_use]
pub fn detail_for(status: Status, message: Option<&str>) -> String {
    match (status, message) {
        (Status::NeedsYou, Some(message)) if !message.is_empty() => message.to_string(),
        (Status::NeedsYou, _) => "Needs you".to_string(),
        (Status::Working | Status::Finished | Status::Failed, Some(message))
            if !message.is_empty() =>
        {
            message.to_string()
        }
        (Status::Failed, _) => "Failed".to_string(),
        (Status::Interrupted, Some(agent)) if !agent.is_empty() => {
            format!("Interrupted. Click to resume {agent}")
        }
        (Status::Interrupted, _) => "Interrupted".to_string(),
        (Status::Working, _) => "Working".to_string(),
        (Status::Finished, _) => "Finished".to_string(),
        (Status::Idle, _) => "Ready".to_string(),
        (Status::Exited, _) => "Exited".to_string(),
        (Status::Background, _) => "Running in background".to_string(),
    }
}

/// What a placed item shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// A heading, by index of the first row under it.
    Section(usize),
    /// A session row, by index.
    Row(usize),
}

/// One item with its vertical position, already scrolled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    /// What it shows.
    pub kind: ItemKind,
    /// Top edge.
    pub top: f32,
    /// Bottom edge.
    pub bottom: f32,
}

/// Group rows by project, in order of first appearance, with host sessions in
/// their own section last. Stable, so rows don't jump around while status changes.
pub fn group(rows: &mut [Row]) {
    let mut seen: Vec<(bool, String)> = Vec::new();
    let mut order = |row: &Row| {
        let key = (is_host(row), row.project.clone());
        let at = seen.iter().position(|k| *k == key).unwrap_or_else(|| {
            seen.push(key);
            seen.len() - 1
        });
        (is_host(row), at)
    };
    let keys: Vec<(bool, usize)> = rows.iter().map(&mut order).collect();
    let mut indexed: Vec<usize> = (0..rows.len()).collect();
    indexed.sort_by_key(|&i| keys[i]);
    let sorted: Vec<Row> = indexed.iter().map(|&i| rows[i].clone()).collect();
    rows.clone_from_slice(&sorted);
}

const fn is_host(row: &Row) -> bool {
    matches!(row.target, Target::Host(_))
}

/// Whether row `i` starts a new heading.
fn starts_section(rows: &[Row], i: usize) -> bool {
    match i.checked_sub(1).and_then(|prev| rows.get(prev)) {
        Some(prev) => prev.project != rows[i].project || is_host(prev) != is_host(&rows[i]),
        None => true,
    }
}

/// Lay out `rows`, already grouped, starting below the header at `top`.
pub fn layout_into(rows: &[Row], top: f32, scroll: f32, out: &mut Vec<Placed>) {
    out.clear();
    let mut y = top + NEW_ROW_HEIGHT - scroll;
    let mut place = |kind, height| {
        out.push(Placed {
            kind,
            top: y,
            bottom: y + height,
        });
        y += height;
    };
    for i in 0..rows.len() {
        if starts_section(rows, i) {
            place(ItemKind::Section(i), SECTION_HEIGHT);
        }
        place(ItemKind::Row(i), ROW_HEIGHT);
    }
}

/// Total height of the scrolling list.
#[must_use]
pub fn content_height(rows: &[Row]) -> f32 {
    (0..rows.len())
        .map(|i| {
            if starts_section(rows, i) {
                SECTION_HEIGHT + ROW_HEIGHT
            } else {
                ROW_HEIGHT
            }
        })
        .sum()
}

/// Keep a scroll offset inside the content.
#[must_use]
pub fn clamp_scroll(scroll: f32, rows: &[Row], visible: f32) -> f32 {
    scroll.clamp(0.0, (content_height(rows) - visible).max(0.0))
}

/// What a point in the sidebar is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// The "New session" row.
    New,
    /// A session row, by index.
    Row(usize),
    /// A row's close target, by index.
    Close(usize),
    /// Nothing clickable.
    None,
}

/// Classify a point. `top` is where the "New session" row starts.
#[must_use]
pub fn hit_test(
    x: f32,
    y: f32,
    top: f32,
    rows: &[Row],
    placed: &[Placed],
    close_button: bool,
) -> Hit {
    if y < top {
        return Hit::None;
    }
    // The list scrolls under this row, so it answers first.
    if y < top + NEW_ROW_HEIGHT {
        return Hit::New;
    }
    let found = placed.iter().find(|p| y >= p.top && y < p.bottom);
    let Some(ItemKind::Row(i)) = found.map(|p| p.kind) else {
        return Hit::None;
    };
    let closable = close_button && rows.get(i).is_some_and(|row| !is_host(row));
    if closable && x >= RAIL_WIDTH - CLOSE_WIDTH {
        Hit::Close(i)
    } else {
        Hit::Row(i)
    }
}

/// The sidebar's width for a window: nothing when off or too narrow.
#[must_use]
pub fn effective_width(enabled: bool, client_width: f32) -> f32 {
    if enabled && client_width >= RAIL_MIN_CLIENT {
        RAIL_WIDTH
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    fn ms(n: u64) -> Option<Duration> {
        Some(Duration::from_millis(n))
    }

    fn row(project: &str, title: &str, target: Target) -> Row {
        Row {
            target,
            title: title.to_string(),
            detail: String::new(),
            project: project.to_string(),
            status: Status::Idle,
            active: false,
        }
    }

    #[test]
    fn spinner_titles_are_recognised() {
        // Break caught: an agent CLI busy for minutes showing as idle.
        for title in [
            "\u{2802} fixing the bug",
            "\u{2810} Claude",
            "\u{280b} codex",
            "\u{2733}\u{fe0f} no",
        ] {
            let want = !title.starts_with('\u{2733}');
            assert_eq!(has_spinner(title), want, "{title}");
        }
        for title in [
            "pwsh in winttyghost",
            "",
            "- dashes",
            "\u{2733} Claude Code",
        ] {
            assert!(!has_spinner(title), "{title}");
        }
    }

    #[test]
    fn each_signal_counts_as_working() {
        assert!(is_working(&Signals {
            title: "\u{2802} task",
            ..Signals::default()
        }));
        assert!(is_working(&Signals {
            command_running: true,
            ..Signals::default()
        }));
        assert!(is_working(&Signals {
            progress: true,
            ..Signals::default()
        }));
        assert!(is_working(&Signals {
            output_age: ms(200),
            ..Signals::default()
        }));
        assert!(is_working(&Signals {
            output_age: ms(200),
            input_age: Some(S * 5),
            ..Signals::default()
        }));
    }

    #[test]
    fn a_quiet_session_is_not_working() {
        assert!(!is_working(&Signals::default()));
        assert!(!is_working(&Signals {
            output_age: Some(S * 3),
            ..Signals::default()
        }));
    }

    #[test]
    fn typing_is_not_working() {
        // Break caught: the row flickering to Working on every keystroke echo.
        assert!(!is_working(&Signals {
            output_age: ms(10),
            input_age: ms(100),
            ..Signals::default()
        }));
    }

    #[test]
    fn an_exited_session_is_never_working() {
        let s = Signals {
            exited: true,
            command_running: true,
            output_age: ms(10),
            ..Signals::default()
        };
        assert!(!is_working(&s));
        assert_eq!(Activity::default().step(&s, false, 0), Status::Exited);
    }

    #[test]
    fn attention_wins_over_work_until_seen() {
        let s = Signals {
            attention: true,
            command_running: true,
            ..Signals::default()
        };
        let mut a = Activity::default();
        assert_eq!(a.step(&s, false, 0), Status::NeedsYou);
        // Once it is on screen, it is just working again.
        assert_eq!(a.step(&s, true, 100), Status::Working);
    }

    fn working<'a>() -> Signals<'a> {
        Signals {
            command_running: true,
            ..Signals::default()
        }
    }

    #[test]
    fn long_work_finishing_unseen_is_marked_until_viewed() {
        // Break caught: coming back to a tab and not knowing the agent is done.
        let mut a = Activity::default();
        assert_eq!(a.step(&working(), false, 0), Status::Working);
        assert_eq!(a.step(&Signals::default(), false, 5000), Status::Finished);
        assert_eq!(
            a.step(&Signals::default(), false, 6000),
            Status::Finished,
            "it stays"
        );
        assert_eq!(
            a.step(&Signals::default(), true, 7000),
            Status::Idle,
            "seen"
        );
        assert_eq!(
            a.step(&Signals::default(), false, 8000),
            Status::Idle,
            "stays seen"
        );
    }

    #[test]
    fn short_work_is_not_marked() {
        // Break caught: every `ls` in a background tab demanding attention.
        let mut a = Activity::default();
        a.step(&working(), false, 0);
        assert_eq!(a.step(&Signals::default(), false, 1000), Status::Idle);
    }

    #[test]
    fn work_finishing_in_view_is_not_marked() {
        let mut a = Activity::default();
        a.step(&working(), true, 0);
        assert_eq!(a.step(&Signals::default(), true, 9000), Status::Idle);
    }

    #[test]
    fn starting_again_clears_the_mark() {
        let mut a = Activity::default();
        a.step(&working(), false, 0);
        a.step(&Signals::default(), false, 5000);
        assert_eq!(a.step(&working(), false, 6000), Status::Working);
    }

    #[test]
    fn a_tab_shows_its_most_urgent_pane() {
        use Status::{Exited, Finished, Idle, NeedsYou, Working};
        assert_eq!(most_urgent([Idle, Working]), Working);
        assert_eq!(most_urgent([Working, NeedsYou, Idle]), NeedsYou);
        assert_eq!(most_urgent([Idle, Finished]), Finished);
        assert_eq!(most_urgent([Exited, Idle]), Idle);
        assert_eq!(most_urgent([Exited]), Exited);
        assert_eq!(most_urgent([]), Idle);
    }

    #[test]
    fn clean_title_drops_status_marks() {
        assert_eq!(clean_title("\u{2802} fixing the bug"), "fixing the bug");
        assert_eq!(clean_title("\u{2733} Claude Code"), "Claude Code");
        assert_eq!(clean_title("pwsh in winttyghost"), "pwsh in winttyghost");
        assert_eq!(clean_title("  spaced"), "spaced");
    }

    #[test]
    fn project_is_the_last_path_component() {
        for (cwd, want) in [
            (Some(r"C:\Users\me\src\rustty"), "rustty"),
            (Some(r"C:\Users\me\src\rustty\"), "rustty"),
            (Some("/mnt/c/work/api/"), "api"),
            (Some("/home/me"), "me"),
            (Some(r"C:\"), r"C:\"),
            (Some("/"), "/"),
            (Some(""), "Other"),
            (None, "Other"),
        ] {
            assert_eq!(project_of(cwd), want, "{cwd:?}");
        }
    }

    #[test]
    fn notification_text_is_made_safe() {
        // Break caught: terminal output drawing control characters in the UI.
        assert_eq!(clean_message("needs\x1b[31m input\n"), "needs[31m input");
        assert_eq!(
            clean_message(&"x".repeat(500)).chars().count(),
            MESSAGE_MAX_CHARS
        );
    }

    #[test]
    fn details_describe_the_status() {
        assert_eq!(detail_for(Status::Working, None), "Working");
        assert_eq!(detail_for(Status::NeedsYou, None), "Needs you");
        assert_eq!(
            detail_for(Status::NeedsYou, Some("Approve the edit?")),
            "Approve the edit?"
        );
        assert_eq!(detail_for(Status::Finished, None), "Finished");
        assert_eq!(detail_for(Status::Idle, None), "Ready");
        assert_eq!(detail_for(Status::Exited, None), "Exited");
        assert_eq!(
            detail_for(Status::Background, None),
            "Running in background"
        );
    }

    #[test]
    fn rows_group_by_project_in_first_seen_order_with_hosts_last() {
        let mut rows = vec![
            row("api", "a", Target::Tab(0)),
            row("web", "b", Target::Tab(1)),
            row("api", "c", Target::Tab(2)),
            row("api", "d", Target::Host(1)),
            row("web", "e", Target::Tab(3)),
        ];
        group(&mut rows);
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["a", "c", "b", "e", "d"]);
    }

    #[test]
    fn layout_puts_a_heading_above_each_group() {
        let mut rows = vec![
            row("api", "a", Target::Tab(0)),
            row("api", "b", Target::Tab(1)),
            row("web", "c", Target::Tab(2)),
        ];
        group(&mut rows);
        let mut placed = Vec::new();
        layout_into(&rows, 100.0, 0.0, &mut placed);
        let kinds: Vec<ItemKind> = placed.iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            [
                ItemKind::Section(0),
                ItemKind::Row(0),
                ItemKind::Row(1),
                ItemKind::Section(2),
                ItemKind::Row(2)
            ]
        );
        assert_eq!(placed[0].top, 100.0 + NEW_ROW_HEIGHT);
        assert_eq!(placed[1].top, 100.0 + NEW_ROW_HEIGHT + SECTION_HEIGHT);
        assert_eq!(placed[1].bottom - placed[1].top, ROW_HEIGHT);
        assert_eq!(
            content_height(&rows),
            2.0 * SECTION_HEIGHT + 3.0 * ROW_HEIGHT
        );
    }

    #[test]
    fn hosts_get_their_own_heading_even_in_the_same_project() {
        let mut rows = vec![
            row("api", "a", Target::Tab(0)),
            row("api", "b", Target::Host(1)),
        ];
        group(&mut rows);
        let mut placed = Vec::new();
        layout_into(&rows, 0.0, 0.0, &mut placed);
        let sections = placed
            .iter()
            .filter(|p| matches!(p.kind, ItemKind::Section(_)))
            .count();
        assert_eq!(sections, 2);
    }

    #[test]
    fn scrolling_moves_the_list_but_not_new_session() {
        let rows = vec![row("api", "a", Target::Tab(0))];
        let mut placed = Vec::new();
        layout_into(&rows, 0.0, 10.0, &mut placed);
        assert_eq!(placed[0].top, NEW_ROW_HEIGHT - 10.0);
        assert_eq!(hit_test(20.0, 5.0, 0.0, &rows, &placed, true), Hit::New);
    }

    #[test]
    fn scroll_stays_inside_the_content() {
        let rows: Vec<Row> = (0..20).map(|i| row("api", "a", Target::Tab(i))).collect();
        let total = content_height(&rows);
        assert_eq!(clamp_scroll(-5.0, &rows, 100.0), 0.0);
        assert_eq!(clamp_scroll(1e6, &rows, 100.0), total - 100.0);
        assert_eq!(clamp_scroll(50.0, &rows, 100.0), 50.0);
        assert_eq!(
            clamp_scroll(50.0, &rows, total + 10.0),
            0.0,
            "everything fits"
        );
    }

    #[test]
    fn hit_test_finds_rows_and_close_targets() {
        let rows = vec![
            row("api", "a", Target::Tab(0)),
            row("api", "b", Target::Host(3)),
        ];
        let mut placed = Vec::new();
        layout_into(&rows, 0.0, 0.0, &mut placed);
        let first = placed
            .iter()
            .find(|p| p.kind == ItemKind::Row(0))
            .expect("row");
        let host = placed
            .iter()
            .find(|p| p.kind == ItemKind::Row(1))
            .expect("row");
        assert_eq!(
            hit_test(20.0, first.top + 1.0, 0.0, &rows, &placed, true),
            Hit::Row(0)
        );
        assert_eq!(
            hit_test(RAIL_WIDTH - 5.0, first.top + 1.0, 0.0, &rows, &placed, true),
            Hit::Close(0)
        );
        // A heading is not clickable.
        assert_eq!(
            hit_test(20.0, placed[0].top + 1.0, 0.0, &rows, &placed, true),
            Hit::None
        );
        // A background session has no close target: closing is not detaching.
        assert_eq!(
            hit_test(RAIL_WIDTH - 5.0, host.top + 1.0, 0.0, &rows, &placed, true),
            Hit::Row(1)
        );
    }

    #[test]
    fn a_row_scrolled_under_new_session_is_not_clickable_there() {
        // Break caught: clicking "New session" activating a row scrolled under it.
        let rows = vec![row("api", "a", Target::Tab(0))];
        let mut placed = Vec::new();
        layout_into(&rows, 0.0, 40.0, &mut placed);
        assert_eq!(hit_test(20.0, 5.0, 0.0, &rows, &placed, true), Hit::New);
    }

    #[test]
    fn sidebar_collapses_when_off_or_narrow() {
        assert_eq!(effective_width(true, 1000.0), RAIL_WIDTH);
        assert_eq!(effective_width(true, RAIL_MIN_CLIENT), RAIL_WIDTH);
        assert_eq!(effective_width(true, RAIL_MIN_CLIENT - 1.0), 0.0);
        assert_eq!(effective_width(false, 1000.0), 0.0);
    }

    #[test]
    fn repaint_after_a_resize_is_not_work() {
        // Break caught: dragging the window marking every session Working,
        // then Finished, when nothing ran.
        assert!(output_is_work(None));
        assert!(!output_is_work(Some(Duration::ZERO)));
        assert!(!output_is_work(Some(
            RESIZE_REPAINT - Duration::from_millis(1)
        )));
        assert!(output_is_work(Some(RESIZE_REPAINT)));
    }

    #[test]
    fn opening_a_background_session_lets_you_type_unless_someone_else_is() {
        // Break caught: a click opening a read-only view of an idle session,
        // or taking the keyboard from another window.
        assert!(opens_with_control(Some(0)));
        assert!(!opens_with_control(Some(1)));
        assert!(!opens_with_control(None));
    }

    #[test]
    fn a_view_label_keeps_the_session_name() {
        // Break caught: "overnight-build" turning into "Session 3" on click.
        assert_eq!(
            view_label(3, Some("overnight-build"), true),
            "overnight-build"
        );
        assert_eq!(
            view_label(3, Some("overnight-build"), false),
            "overnight-build · watch"
        );
        assert_eq!(view_label(3, None, true), "Session 3");
        assert_eq!(view_label(3, Some(""), false), "Session 3 · watch");
    }

    #[test]
    fn a_spinner_left_behind_stops_counting_as_work() {
        // Break caught: a crashed agent's title showing Working forever, and
        // the sidebar repainting for it.
        let signals = |age| Signals {
            title: "\u{2802} Fixing",
            output_age: age,
            ..Signals::default()
        };
        assert!(is_working(&signals(None)));
        assert!(is_working(&signals(Some(
            SPINNER_STALE - Duration::from_secs(1)
        ))));
        assert!(!is_working(&signals(Some(SPINNER_STALE))));
    }

    #[test]
    fn progress_left_behind_stops_counting_as_work() {
        let signals = |age| Signals {
            progress: true,
            output_age: age,
            ..Signals::default()
        };
        assert!(is_working(&signals(Some(
            PROGRESS_STALE - Duration::from_secs(1)
        ))));
        assert!(!is_working(&signals(Some(PROGRESS_STALE))));
    }

    #[test]
    fn a_pulse_has_few_steps_inside_its_range() {
        // Break caught: every frame minting a new colour and repainting.
        let mut seen = Vec::new();
        for ms in 0..5000 {
            let level = pulse_at(ms as f32 / 1000.0);
            assert!((0.45..=1.0).contains(&level), "{level}");
            if !seen.contains(&level) {
                seen.push(level);
            }
        }
        assert!(seen.len() > 3, "{seen:?}");
        assert!(seen.len() <= PULSE_LEVELS as usize + 1, "{seen:?}");
    }

    #[test]
    fn reset_forgets_work_that_nothing_was_watching() {
        // Break caught: showing the sidebar again reporting a false Finished.
        let mut activity = Activity::default();
        let busy = Signals {
            progress: true,
            output_age: Some(Duration::ZERO),
            ..Signals::default()
        };
        assert_eq!(activity.step(&busy, false, 0), Status::Working);
        activity.reset();
        assert_eq!(
            activity.step(&Signals::default(), false, 10_000),
            Status::Idle
        );
    }

    #[test]
    fn row_text_cannot_break_the_row() {
        // Break caught: a title with newlines drawing over other rows, and a
        // 64 KB title going through DrawText every frame.
        assert_eq!(row_title("\u{2802} a\nb\x1b", "pwsh"), "ab");
        assert_eq!(row_title("\u{2802} \n", "pwsh"), "pwsh");
        assert_eq!(row_title("", "pwsh"), "pwsh");
        assert_eq!(
            row_title(&"x".repeat(5000), "pwsh").chars().count(),
            MESSAGE_MAX_CHARS
        );
        assert_eq!(project_of(Some("C:\\src\\ap\ni")), "api");
    }

    #[test]
    fn close_asks_when_the_row_shows_work() {
        // Break caught: a stray click killing a running agent.
        for status in [Status::Working, Status::NeedsYou] {
            assert!(
                close_needs_confirm(status, false, true, false),
                "{status:?}"
            );
        }
        assert!(!close_needs_confirm(Status::Idle, false, true, false));
        assert!(!close_needs_confirm(Status::Finished, false, true, false));
        assert!(close_needs_confirm(Status::Idle, true, true, false));
        assert!(close_needs_confirm(Status::Exited, true, false, true));
        assert!(!close_needs_confirm(Status::Exited, true, false, false));
    }

    #[test]
    fn close_button_off_makes_the_whole_row_a_row() {
        // Break caught: close-button = false and a click still closing a tab.
        let rows = vec![row("api", "a", Target::Tab(0))];
        let mut placed = Vec::new();
        layout_into(&rows, 0.0, 0.0, &mut placed);
        let top = placed
            .iter()
            .find(|p| matches!(p.kind, ItemKind::Row(0)))
            .map_or(0.0, |p| p.top + 1.0);
        let edge = RAIL_WIDTH - 1.0;
        assert_eq!(
            hit_test(edge, top, 0.0, &rows, &placed, true),
            Hit::Close(0)
        );
        assert_eq!(hit_test(edge, top, 0.0, &rows, &placed, false), Hit::Row(0));
    }

    #[test]
    fn a_grid_drag_clamps_at_the_sidebar() {
        // Break caught: a selection stopping short of column 0, and a mouse
        // release a TUI never receives.
        assert_eq!(drag_x(10.0, RAIL_WIDTH, true), RAIL_WIDTH);
        assert_eq!(drag_x(300.0, RAIL_WIDTH, true), 300.0);
        assert_eq!(drag_x(10.0, RAIL_WIDTH, false), 10.0);
    }

    #[test]
    fn an_interrupted_row_says_what_a_click_does() {
        assert_eq!(
            detail_for(Status::Interrupted, Some("claude")),
            "Interrupted. Click to resume claude"
        );
        assert_eq!(detail_for(Status::Interrupted, None), "Interrupted");
        assert_eq!(
            most_urgent([Status::Working, Status::Interrupted, Status::Idle]),
            Status::Interrupted
        );
        assert_eq!(
            most_urgent([Status::Interrupted, Status::NeedsYou]),
            Status::NeedsYou
        );
    }

    #[test]
    fn a_worktree_session_groups_with_its_project() {
        // Break caught: every worker listed as its own project.
        assert_eq!(project_of(Some("C:/src/api/.worktrees/fix-login")), "api");
        assert_eq!(
            project_of(Some("C:\\src\\api\\.worktrees\\fix-login\\src")),
            "api"
        );
        assert_eq!(project_of(Some("C:/src/api")), "api");
    }

    #[test]
    fn a_reported_message_shows_on_the_row() {
        assert_eq!(
            detail_for(Status::Working, Some("Running tests")),
            "Running tests"
        );
        assert_eq!(detail_for(Status::Working, None), "Working");
        assert_eq!(
            detail_for(Status::Failed, Some("Build broke")),
            "Build broke"
        );
        assert_eq!(detail_for(Status::Failed, None), "Failed");
        assert_eq!(detail_for(Status::Finished, Some("All green")), "All green");
        assert_eq!(
            most_urgent([Status::Working, Status::Failed]),
            Status::Failed
        );
        assert_eq!(
            most_urgent([Status::Failed, Status::NeedsYou]),
            Status::NeedsYou
        );
    }

    #[test]
    fn a_circle_spinner_is_not_part_of_the_title() {
        assert_eq!(clean_title("\u{25D0} Claude Code"), "Claude Code");
    }

    #[test]
    fn a_circle_spinner_counts_as_a_spinner() {
        assert!(has_spinner("\u{25D0} Claude Code"));
        assert!(has_spinner("\u{25D3} x"));
        assert!(!has_spinner("\u{2733} Claude Code"));
    }
}
