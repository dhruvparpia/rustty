//! The project roadmap board (BOARD-001): the columns and cards in a project's
//! `roadmap.md`. The file stays plain Markdown that people and agents can edit,
//! so this keeps every line it does not understand.

use std::path::{Path, PathBuf};

/// The board's columns, as `##` headings in the file.
pub const COLUMNS: [&str; 4] = ["Idea bank", "Backlog", "In progress", "Done"];
/// The column a card moves to when a session opens for it.
pub const IN_PROGRESS: usize = 2;
/// The longest card title kept, in characters.
const MAX_TITLE_CHARS: usize = 200;

/// One line or card under a heading.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    /// A list item, with the lines indented under it.
    Card { title: String, raw: Vec<String> },
    /// Anything else, kept as written.
    Text(String),
}

/// A heading and what follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Section {
    /// The heading line, `None` for what comes before the first heading.
    heading: Option<String>,
    /// The board column this heading is.
    column: Option<usize>,
    entries: Vec<Entry>,
}

/// A parsed `roadmap.md`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Board {
    sections: Vec<Section>,
}

/// A card's title from a list line, without its checkbox.
fn card_title(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))?;
    let rest = ["[ ] ", "[x] ", "[X] "]
        .iter()
        .find_map(|mark| rest.strip_prefix(mark))
        .unwrap_or(rest);
    let title = rest.trim();
    (!title.is_empty()).then(|| title.to_string())
}

/// Make typed or dropped text safe to keep as one card line.
#[must_use]
pub fn clean_card(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect()
}

impl Board {
    /// Parse the file's text. Never fails: unknown lines are kept.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut sections = vec![Section {
            heading: None,
            column: None,
            entries: Vec::new(),
        }];
        for line in text.lines() {
            if let Some(name) = line.strip_prefix("## ") {
                let column = COLUMNS
                    .iter()
                    .position(|c| c.eq_ignore_ascii_case(name.trim()));
                sections.push(Section {
                    heading: Some(line.to_string()),
                    column,
                    entries: Vec::new(),
                });
                continue;
            }
            let Some(section) = sections.last_mut() else {
                continue;
            };
            if section.column.is_some() {
                if let Some(title) = card_title(line) {
                    section.entries.push(Entry::Card {
                        title,
                        raw: vec![line.to_string()],
                    });
                    continue;
                }
                if line.starts_with([' ', '\t'])
                    && !line.trim().is_empty()
                    && let Some(Entry::Card { raw, .. }) = section.entries.last_mut()
                {
                    raw.push(line.to_string());
                    continue;
                }
            }
            section.entries.push(Entry::Text(line.to_string()));
        }
        Self { sections }
    }

    /// The file's text.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        for section in &self.sections {
            if let Some(heading) = &section.heading {
                out.push_str(heading);
                out.push('\n');
            }
            for entry in &section.entries {
                match entry {
                    Entry::Card { raw, .. } => {
                        for line in raw {
                            out.push_str(line);
                            out.push('\n');
                        }
                    }
                    Entry::Text(line) => {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
            }
        }
        out
    }

    /// The card titles in a column.
    #[must_use]
    pub fn cards(&self, column: usize) -> Vec<&str> {
        self.sections
            .iter()
            .filter(|s| s.column == Some(column))
            .flat_map(|s| &s.entries)
            .filter_map(|entry| match entry {
                Entry::Card { title, .. } => Some(title.as_str()),
                Entry::Text(_) => None,
            })
            .collect()
    }

    /// The section holding `column`, created at the end when missing.
    fn section_mut(&mut self, column: usize) -> &mut Section {
        let at = self.sections.iter().position(|s| s.column == Some(column));
        let at = at.unwrap_or_else(|| {
            self.sections.push(Section {
                heading: Some(format!("## {}", COLUMNS[column])),
                column: Some(column),
                entries: Vec::new(),
            });
            self.sections.len() - 1
        });
        &mut self.sections[at]
    }

    /// Where a card goes in a section: after the last card, before the blank
    /// lines that separate it from the next heading.
    fn insert_at(section: &Section) -> usize {
        section
            .entries
            .iter()
            .rposition(|e| matches!(e, Entry::Card { .. }))
            .map_or_else(
                || {
                    section
                        .entries
                        .iter()
                        .position(|e| matches!(e, Entry::Text(t) if t.trim().is_empty()))
                        .unwrap_or(section.entries.len())
                },
                |last| last + 1,
            )
    }

    /// Add a card to a column. `false` when the title is empty after cleaning.
    pub fn add(&mut self, column: usize, title: &str) -> bool {
        let title = clean_card(title);
        if title.is_empty() || column >= COLUMNS.len() {
            return false;
        }
        let section = self.section_mut(column);
        let at = Self::insert_at(section);
        section.entries.insert(
            at,
            Entry::Card {
                raw: vec![format!("- {title}")],
                title,
            },
        );
        true
    }

    /// Move card `index` of column `from` to the end of column `to`, lines
    /// under it included. Returns its new index.
    pub fn move_card(&mut self, from: usize, index: usize, to: usize) -> Option<usize> {
        if to >= COLUMNS.len() {
            return None;
        }
        let mut seen = 0;
        let mut taken = None;
        for section in self.sections.iter_mut().filter(|s| s.column == Some(from)) {
            for at in 0..section.entries.len() {
                if matches!(section.entries[at], Entry::Card { .. }) {
                    if seen == index {
                        taken = Some(section.entries.remove(at));
                        break;
                    }
                    seen += 1;
                }
            }
            if taken.is_some() {
                break;
            }
        }
        let card = taken?;
        let section = self.section_mut(to);
        let at = Self::insert_at(section);
        section.entries.insert(at, card);
        Some(self.cards(to).len() - 1)
    }
}

/// A key on the open board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Selection left.
    Left,
    /// Selection right.
    Right,
    /// Selection up.
    Up,
    /// Selection down.
    Down,
    /// Move the card a column left.
    MoveLeft,
    /// Move the card a column right.
    MoveRight,
    /// Enter.
    Enter,
    /// Esc.
    Esc,
    /// Backspace.
    Backspace,
    /// A typed character.
    Char(char),
}

/// What the window should do after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing outside the board.
    None,
    /// Close the board.
    Close,
    /// The cards changed: write the file.
    Save,
    /// Open a session for this card and write the file.
    OpenSession(String),
    /// Open a session for this card in its own worktree and write the file.
    OpenWorktree(String),
}

/// The open board: its document and where the person is on it.
#[derive(Debug, Clone, Default)]
pub struct State {
    /// The document.
    pub board: Board,
    /// Selected column.
    pub column: usize,
    /// Selected card in that column.
    pub row: usize,
    /// The new card being typed.
    pub editing: Option<String>,
}

impl State {
    /// Keep the selection on a card after the document changed.
    pub fn clamp(&mut self) {
        self.column = self.column.min(COLUMNS.len() - 1);
        self.row = self
            .row
            .min(self.board.cards(self.column).len().saturating_sub(1));
    }

    /// Take the selected card to the In progress column for a session.
    fn start_selected(&mut self) -> Option<String> {
        let title = self.board.cards(self.column).get(self.row)?.to_string();
        if self.column != IN_PROGRESS {
            let row = self.board.move_card(self.column, self.row, IN_PROGRESS)?;
            self.column = IN_PROGRESS;
            self.row = row;
        }
        Some(title)
    }

    /// Handle a key.
    pub fn key(&mut self, key: Key) -> Action {
        if let Some(text) = &mut self.editing {
            match key {
                Key::Esc => self.editing = None,
                Key::Backspace => {
                    text.pop();
                }
                Key::Char(c) if !c.is_control() => text.push(c),
                Key::Enter => {
                    let title = self.editing.take().unwrap_or_default();
                    if self.board.add(self.column, &title) {
                        self.row = self.board.cards(self.column).len() - 1;
                        return Action::Save;
                    }
                }
                _ => {}
            }
            return Action::None;
        }
        match key {
            Key::Esc => return Action::Close,
            Key::Left => {
                self.column = self.column.saturating_sub(1);
                self.clamp();
            }
            Key::Right => {
                self.column += 1;
                self.clamp();
            }
            Key::Up => self.row = self.row.saturating_sub(1),
            Key::Down => {
                self.row += 1;
                self.clamp();
            }
            Key::MoveLeft | Key::MoveRight => {
                let to = if key == Key::MoveLeft {
                    self.column.checked_sub(1)
                } else {
                    Some(self.column + 1).filter(|&to| to < COLUMNS.len())
                };
                if let Some(to) = to
                    && let Some(row) = self.board.move_card(self.column, self.row, to)
                {
                    self.column = to;
                    self.row = row;
                    return Action::Save;
                }
            }
            Key::Enter => {
                if let Some(title) = self.start_selected() {
                    return Action::OpenSession(title);
                }
            }
            Key::Char('w' | 'W') => {
                if let Some(title) = self.start_selected() {
                    return Action::OpenWorktree(title);
                }
            }
            Key::Char('n' | 'N') => self.editing = Some(String::new()),
            Key::Backspace | Key::Char(_) => {}
        }
        Action::None
    }
}

/// What the renderer draws.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct View {
    /// The project's name.
    pub project: String,
    /// Card titles per column.
    pub columns: [Vec<String>; COLUMNS.len()],
    /// Selected column.
    pub column: usize,
    /// Selected card.
    pub row: usize,
    /// The new card being typed.
    pub editing: Option<String>,
}

impl State {
    /// The picture of this state.
    #[must_use]
    pub fn view(&self, project: &str) -> View {
        let mut columns: [Vec<String>; COLUMNS.len()] = Default::default();
        for (at, column) in columns.iter_mut().enumerate() {
            *column = self
                .board
                .cards(at)
                .iter()
                .map(|c| (*c).to_string())
                .collect();
        }
        View {
            project: project.to_string(),
            columns,
            column: self.column,
            row: self.row,
            editing: self.editing.clone(),
        }
    }
}

/// Height of the board's heading row.
pub const HEADER_HEIGHT: f32 = 44.0;
/// Height of a column heading.
pub const COLUMN_HEADER_HEIGHT: f32 = 32.0;
/// Height of a card, gap included.
pub const CARD_HEIGHT: f32 = 44.0;
/// Space around columns.
pub const GUTTER: f32 = 12.0;

/// The width of one column in an area this wide.
#[must_use]
pub fn column_width(width: f32) -> f32 {
    ((width - GUTTER) / COLUMNS.len() as f32).max(0.0)
}

/// The first card to draw in a column, so the selected one is on screen.
#[must_use]
pub fn first_visible(view: &View, column: usize, height: f32) -> usize {
    if column != view.column {
        return 0;
    }
    let room = ((height - HEADER_HEIGHT - COLUMN_HEADER_HEIGHT) / CARD_HEIGHT).floor();
    let room = if room >= 1.0 { room as usize } else { 1 };
    (view.row + 1).saturating_sub(room)
}

/// What a point in the board's area is over, relative to the area's corner.
#[must_use]
pub fn hit(view: &View, x: f32, y: f32, width: f32, height: f32) -> Option<(usize, Option<usize>)> {
    let w = column_width(width);
    if w <= 0.0 || x < GUTTER {
        return None;
    }
    let column = ((x - GUTTER) / w) as usize;
    if column >= COLUMNS.len() || y < HEADER_HEIGHT {
        return None;
    }
    let below = y - HEADER_HEIGHT - COLUMN_HEADER_HEIGHT;
    let row = (below >= 0.0)
        .then(|| (below / CARD_HEIGHT) as usize + first_visible(view, column, height))
        .filter(|&row| row < view.columns[column].len());
    Some((column, row))
}

/// Write the file whole or not at all, creating its folder.
///
/// # Errors
/// The folder or file cannot be written.
pub fn write_file(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut temp = PathBuf::from(path);
    temp.set_extension("md.tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "# Roadmap\n\nWhat we are building.\n\n## Idea bank\n- First idea\n- [ ] Second idea\n  details under it\n\n## Backlog\n\n## In progress\n- Working on it\n\n## Done\n- [x] Shipped it\n\n## Notes\n- not a card, not a column\n";

    fn state() -> State {
        State {
            board: Board::parse(FILE),
            ..State::default()
        }
    }

    #[test]
    fn a_file_comes_back_exactly_as_it_was() {
        // Break caught: opening the board rewriting someone's notes.
        assert_eq!(Board::parse(FILE).render(), FILE);
    }

    #[test]
    fn cards_are_read_from_their_columns() {
        let board = Board::parse(FILE);
        assert_eq!(board.cards(0), vec!["First idea", "Second idea"]);
        assert_eq!(board.cards(1), Vec::<&str>::new());
        assert_eq!(board.cards(2), vec!["Working on it"]);
        assert_eq!(board.cards(3), vec!["Shipped it"]);
    }

    #[test]
    fn moving_a_card_takes_its_lines_and_leaves_the_rest() {
        let mut board = Board::parse(FILE);
        assert_eq!(board.move_card(0, 1, 1), Some(0));
        assert_eq!(board.cards(0), vec!["First idea"]);
        assert_eq!(board.cards(1), vec!["Second idea"]);
        let text = board.render();
        assert!(
            text.contains("## Backlog\n- [ ] Second idea\n  details under it\n"),
            "{text}"
        );
        assert!(
            text.contains("## Notes\n- not a card, not a column\n"),
            "{text}"
        );
        assert!(text.contains("What we are building."), "{text}");
    }

    #[test]
    fn a_file_with_no_columns_gets_them_when_needed() {
        let mut board = Board::parse("just some text\n");
        assert!(board.add(1, "A card"));
        let text = board.render();
        assert!(text.starts_with("just some text\n"), "{text}");
        assert!(text.contains("## Backlog\n- A card\n"), "{text}");
        assert_eq!(Board::parse(&text).cards(1), vec!["A card"]);
    }

    #[test]
    fn card_text_stays_on_one_line() {
        // Break caught: a pasted newline turning into a fake heading.
        let mut board = Board::parse("");
        assert!(board.add(0, "one\n## Done\ntwo"));
        assert_eq!(board.cards(0), vec!["one ## Done two"]);
        assert_eq!(board.cards(3), Vec::<&str>::new());
        assert!(!board.add(0, " \n "));
        assert_eq!(clean_card("a\x1bb"), "a b", "no escape codes either");
        assert_eq!(
            clean_card(&"x".repeat(999)).chars().count(),
            MAX_TITLE_CHARS
        );
    }

    #[test]
    fn a_stale_move_changes_nothing() {
        let mut board = Board::parse(FILE);
        assert_eq!(board.move_card(0, 9, 1), None);
        assert_eq!(board.move_card(0, 0, 9), None);
        assert_eq!(board.render(), FILE);
    }

    #[test]
    fn arrows_stay_on_the_board() {
        let mut s = state();
        s.key(Key::Left);
        s.key(Key::Up);
        assert_eq!((s.column, s.row), (0, 0));
        s.key(Key::Down);
        s.key(Key::Down);
        s.key(Key::Down);
        assert_eq!((s.column, s.row), (0, 1));
        s.key(Key::Right);
        assert_eq!((s.column, s.row), (1, 0), "an empty column");
        for _ in 0..9 {
            s.key(Key::Right);
        }
        assert_eq!(s.column, 3);
    }

    #[test]
    fn moving_with_keys_follows_the_card() {
        let mut s = state();
        assert_eq!(s.key(Key::MoveRight), Action::Save);
        assert_eq!((s.column, s.row), (1, 0));
        assert_eq!(s.board.cards(1), vec!["First idea"]);
        assert_eq!(s.key(Key::MoveLeft), Action::Save);
        s.column = 3;
        s.row = 0;
        assert_eq!(s.key(Key::MoveRight), Action::None, "no column past Done");
    }

    #[test]
    fn opening_a_session_starts_the_card() {
        let mut s = state();
        assert_eq!(
            s.key(Key::Enter),
            Action::OpenSession("First idea".to_string())
        );
        assert_eq!(s.column, IN_PROGRESS);
        assert_eq!(
            s.board.cards(IN_PROGRESS),
            vec!["Working on it", "First idea"]
        );
        assert_eq!(s.row, 1);
        // Already in progress: it stays put.
        assert_eq!(
            s.key(Key::Char('w')),
            Action::OpenWorktree("First idea".to_string())
        );
        assert_eq!(s.board.cards(IN_PROGRESS).len(), 2);
    }

    #[test]
    fn nothing_to_open_in_an_empty_column() {
        let mut s = state();
        s.column = 1;
        assert_eq!(s.key(Key::Enter), Action::None);
        assert_eq!(s.key(Key::MoveRight), Action::None);
    }

    #[test]
    fn typing_a_new_card() {
        let mut s = state();
        s.column = 1;
        assert_eq!(s.key(Key::Char('n')), Action::None);
        for c in "Abw".chars() {
            assert_eq!(s.key(Key::Char(c)), Action::None, "letters are text now");
        }
        s.key(Key::Backspace);
        assert_eq!(s.editing.as_deref(), Some("Ab"));
        assert_eq!(s.key(Key::Enter), Action::Save);
        assert_eq!(s.editing, None);
        assert_eq!(s.board.cards(1), vec!["Ab"]);
        s.key(Key::Char('n'));
        assert_eq!(s.key(Key::Esc), Action::None, "Esc leaves the typing first");
        assert_eq!(s.key(Key::Esc), Action::Close);
    }

    #[test]
    fn a_click_finds_its_card() {
        let view = state().view("demo");
        let width = 800.0;
        let w = column_width(width);
        let y = HEADER_HEIGHT + COLUMN_HEADER_HEIGHT + 1.0;
        let tall = 600.0;
        assert_eq!(hit(&view, GUTTER + 1.0, y, width, tall), Some((0, Some(0))));
        assert_eq!(
            hit(&view, GUTTER + 1.0, y + CARD_HEIGHT, width, tall),
            Some((0, Some(1)))
        );
        assert_eq!(
            hit(&view, GUTTER + 1.0, y + 2.0 * CARD_HEIGHT, width, tall),
            Some((0, None))
        );
        assert_eq!(
            hit(&view, GUTTER + w + 1.0, y, width, tall),
            Some((1, None))
        );
        assert_eq!(hit(&view, GUTTER + 1.0, 5.0, width, tall), None);
        assert_eq!(hit(&view, 1.0, y, width, tall), None);
    }

    #[test]
    fn a_long_column_scrolls_to_the_selected_card() {
        // Break caught: a selection moving off the bottom of the window, and
        // clicks landing on the wrong card after that.
        let mut view = View {
            column: 1,
            row: 9,
            ..View::default()
        };
        view.columns[1] = (0..10).map(|n| n.to_string()).collect();
        let height = HEADER_HEIGHT + COLUMN_HEADER_HEIGHT + 3.0 * CARD_HEIGHT;
        assert_eq!(first_visible(&view, 1, height), 7);
        assert_eq!(first_visible(&view, 0, height), 0, "other columns stay put");
        let x = GUTTER + column_width(800.0) + 1.0;
        let y = HEADER_HEIGHT + COLUMN_HEADER_HEIGHT + 1.0;
        assert_eq!(hit(&view, x, y, 800.0, height), Some((1, Some(7))));
    }

    #[test]
    fn saving_creates_the_folder_and_replaces_the_file() {
        let dir = std::env::temp_dir().join(format!("rustty-board-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("roadmap.md");
        write_file(&path, "one").expect("first write");
        write_file(&path, "two").expect("second write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "two");
        let leftovers = std::fs::read_dir(path.parent().expect("parent"))
            .expect("list")
            .count();
        assert_eq!(leftovers, 1, "no temp file left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
