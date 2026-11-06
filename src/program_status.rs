//! The Program Status Protocol, OSC 7501 revision 0.4 (STATUS-001): programs tell
//! the terminal outright whether they are working, waiting on the person, done
//! or failed. Claude Code reports this once the terminal answers the query.

/// What rustty answers to the support query. Nothing else is ever sent back.
pub const QUERY_REPLY: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";
/// Longest report, in bytes.
const MAX_SEQUENCE: usize = 4096;
/// Longest key, in bytes.
const MAX_KEY: usize = 16;
/// Longest `msg`: encoded, then decoded.
const MAX_MSG: (usize, usize) = (2732, 2048);
/// Longest `title`: encoded, then decoded.
const MAX_TITLE: (usize, usize) = (256, 192);
/// Longest `app`, in bytes.
const MAX_APP: usize = 32;
/// Longest `id`: whole, per segment, segments.
const MAX_ID: (usize, usize, usize) = (128, 32, 8);
/// Records kept per pane.
const MAX_RECORDS: usize = 64;

/// A program's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// At rest, waiting for the person.
    Idle,
    /// Running.
    Working,
    /// Finished with a result nobody has seen.
    Done,
    /// Waiting on the person.
    Blocked,
    /// Failed and stopped.
    Error,
}

/// One record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Its id: empty is the root record.
    pub id: String,
    /// Its state.
    pub state: State,
    /// How a blocked program is waiting: permission, question or auth.
    pub kind: Option<String>,
    /// One line about what it is doing.
    pub msg: Option<String>,
    /// A short label.
    pub title: Option<String>,
    /// The program's name.
    pub app: Option<String>,
}

/// What a sequence asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// The support query.
    Query,
    /// Replace a record.
    Set(Record),
    /// Remove a record and those under it, or every record when `None`.
    Clear(Option<String>),
    /// Malformed or over a limit: discarded whole.
    Ignored,
}

/// The bytes to write for the support query.
#[must_use]
pub fn query_reply() -> Vec<u8> {
    if cfg!(windows) {
        as_console_keys(QUERY_REPLY)
    } else {
        QUERY_REPLY.to_vec()
    }
}

/// `text` as console key events (`win32-input-mode`, `CSI Vk;Sc;Uc;Kd;Cs;Rc _`),
/// one press and release per character. The Windows console's input layer drops
/// an OSC reply written to it as text, but hands key events to the program, whose
/// standard input then reads the same characters.
#[must_use]
pub fn as_console_keys(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 32);
    for byte in text {
        for down in [1, 0] {
            out.extend_from_slice(format!("\x1b[0;0;{byte};{down};0;1_").as_bytes());
        }
    }
    out
}

/// Parse what follows `OSC 7501 ;`.
#[must_use]
pub fn parse(rest: &str) -> Report {
    if rest == "?" {
        return Report::Query;
    }
    if rest.len() > MAX_SEQUENCE {
        return Report::Ignored;
    }
    let mut clear = false;
    let mut id = None;
    let mut state = None;
    let mut kind = None;
    let mut msg = None;
    let mut title = None;
    let mut app = None;
    for pair in rest.split(':') {
        if pair == "clear" {
            clear = true;
            continue;
        }
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        if key.is_empty() || key.len() > MAX_KEY || !key.bytes().all(|b| b.is_ascii_lowercase()) {
            continue;
        }
        match key {
            "state" => {
                state = match value {
                    "idle" => Some(State::Idle),
                    "working" => Some(State::Working),
                    "done" => Some(State::Done),
                    "blocked" => Some(State::Blocked),
                    "error" => Some(State::Error),
                    _ => None,
                };
            }
            "id" => {
                if !valid_id(value) {
                    return Report::Ignored;
                }
                id = Some(value.to_string());
            }
            "kind" => kind = Some(value.to_string()),
            "app" => {
                let name_char =
                    |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-');
                if value.len() > MAX_APP || !value.bytes().all(name_char) {
                    return Report::Ignored;
                }
                app = Some(value.to_string());
            }
            "msg" => match text_value(value, MAX_MSG) {
                Some(text) => msg = Some(text),
                None => return Report::Ignored,
            },
            "title" => match text_value(value, MAX_TITLE) {
                Some(text) => title = Some(text),
                None => return Report::Ignored,
            },
            _ => {}
        }
    }
    if clear {
        return Report::Clear(id);
    }
    let Some(state) = state else {
        return Report::Ignored;
    };
    Report::Set(Record {
        id: id.unwrap_or_default(),
        state,
        kind: kind.filter(|_| state == State::Blocked),
        msg,
        title,
        app,
    })
}

/// Whether an id fits the limits.
fn valid_id(id: &str) -> bool {
    let (whole, segment, depth) = MAX_ID;
    id.len() <= whole
        && id.split('/').count() <= depth
        && id.split('/').all(|part| {
            part.len() <= segment
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-'))
        })
}

/// A base64 text value, or `None` when it must discard the report.
fn text_value(value: &str, (encoded, decoded): (usize, usize)) -> Option<String> {
    if value.len() > encoded {
        return None;
    }
    let bytes = decode_base64(value)?;
    if bytes.len() > decoded {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    if text.chars().any(char::is_control) {
        return None;
    }
    Some(text.chars().filter(|&c| !is_invisible(c)).collect())
}

/// Characters that change how text reads without showing: direction overrides
/// and invisible formatting.
#[must_use]
pub const fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

/// Decode standard base64, with or without padding.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let body = text.trim_end_matches('=');
    if text.len() - body.len() > 2 {
        return None;
    }
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    let mut bits = 0u32;
    let mut count = 0u32;
    for byte in body.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push(((bits >> count) & 0xFF) as u8);
        }
    }
    (count < 6).then_some(out)
}

/// The records a pane holds. Newest last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Records {
    records: Vec<Record>,
}

impl Records {
    /// Apply a report.
    pub fn apply(&mut self, report: Report) {
        match report {
            Report::Query | Report::Ignored => {}
            Report::Clear(None) => self.records.clear(),
            Report::Clear(Some(id)) => {
                let below = format!("{id}/");
                self.records
                    .retain(|r| r.id != id && !r.id.starts_with(&below));
            }
            Report::Set(record) => {
                self.records.retain(|r| r.id != record.id);
                if self.records.len() >= MAX_RECORDS {
                    self.records.remove(0);
                }
                self.records.push(record);
            }
        }
    }

    /// A new shell prompt, or the program ended: nothing is working or waiting
    /// any more. Results stay until seen.
    pub fn program_gone(&mut self) {
        self.records
            .retain(|r| matches!(r.state, State::Done | State::Error));
    }

    /// The person can see this pane, so its results have been seen.
    pub fn mark_seen(&mut self) {
        self.records
            .retain(|r| !matches!(r.state, State::Done | State::Error));
    }

    /// Whether any record is waiting on a person's permission or sign-in. A program
    /// saying this falsely can only stop a token writing to it (AUTO-007).
    #[must_use]
    pub fn awaits_approval(&self) -> bool {
        self.records.iter().any(|record| {
            record.state == State::Blocked
                && matches!(record.kind.as_deref(), Some("permission" | "auth"))
        })
    }

    /// The record that matters most, if any.
    #[must_use]
    pub fn summary(&self) -> Option<&Record> {
        let rank = |record: &&Record| match record.state {
            State::Blocked => 0,
            State::Error => 1,
            State::Working => 2,
            State::Done => 3,
            State::Idle => 4,
        };
        // Among equals, the newest.
        self.records.iter().rev().min_by_key(rank)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Standard base64, the test's own so it cannot share a bug.
    fn b64(text: &str) -> String {
        const AB: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in text.as_bytes().chunks(3) {
            let n = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            for at in 0..4 {
                if at <= chunk.len() {
                    out.push(char::from(AB[((n >> (18 - 6 * at)) & 63) as usize]));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn set(rest: &str) -> Record {
        match parse(rest) {
            Report::Set(record) => record,
            other => panic!("{rest}: {other:?}"),
        }
    }

    #[test]
    fn the_query_is_recognized_and_answered_as_the_spec_says() {
        assert_eq!(parse("?"), Report::Query);
        let reply = String::from_utf8_lossy(QUERY_REPLY);
        assert!(reply.starts_with("\x1b]7501;?:"), "{reply}");
        assert!(
            reply.contains("states=idle,working,done,blocked,error"),
            "{reply}"
        );
        assert!(reply.contains("kinds=permission,question,auth"), "{reply}");
        assert!(reply.ends_with("\x1b\\"), "{reply}");
    }

    #[test]
    fn every_state_is_read() {
        assert_eq!(set("state=idle").state, State::Idle);
        assert_eq!(set("state=working").state, State::Working);
        assert_eq!(set("state=done").state, State::Done);
        assert_eq!(set("state=blocked").state, State::Blocked);
        assert_eq!(set("state=error").state, State::Error);
        assert_eq!(parse("state=dancing"), Report::Ignored);
        assert_eq!(parse("app=claude"), Report::Ignored, "state is required");
    }

    #[test]
    fn messages_and_labels_are_decoded() {
        let record = set(&format!(
            "state=blocked:kind=permission:app=claude-code:msg={}:title={}",
            b64("Approve the edit?"),
            b64("Refactor")
        ));
        assert_eq!(record.kind.as_deref(), Some("permission"));
        assert_eq!(record.app.as_deref(), Some("claude-code"));
        assert_eq!(record.msg.as_deref(), Some("Approve the edit?"));
        assert_eq!(record.title.as_deref(), Some("Refactor"));
        // Padding is optional.
        let bare = b64("hi").trim_end_matches('=').to_string();
        assert_eq!(
            set(&format!("state=done:msg={bare}")).msg.as_deref(),
            Some("hi")
        );
    }

    #[test]
    fn a_control_character_or_bad_encoding_discards_the_whole_report() {
        // Break caught: a program drawing over the sidebar or faking text.
        for bad in ["line\nbreak", "tab\there", "esc\x1b[31m", "del\x7f"] {
            let rest = format!("state=done:msg={}", b64(bad));
            assert_eq!(parse(&rest), Report::Ignored, "{bad:?}");
        }
        assert_eq!(parse("state=done:msg=@@@"), Report::Ignored);
        let not_utf8 = "state=done:msg=/w==";
        assert_eq!(parse(not_utf8), Report::Ignored);
    }

    #[test]
    fn invisible_and_direction_characters_are_removed() {
        // Spec: neutralize these outside the grid. "evil\u{202E}txt.exe" would
        // otherwise read backwards in the sidebar.
        let record = set(&format!(
            "state=done:msg={}",
            b64("evil\u{202E}txt\u{200B}.exe")
        ));
        assert_eq!(record.msg.as_deref(), Some("eviltxt.exe"));
    }

    #[test]
    fn limits_discard_the_whole_report() {
        let big = "x".repeat(MAX_SEQUENCE);
        assert_eq!(
            parse(&format!("state=done:app=a:zz={big}")),
            Report::Ignored
        );
        assert_eq!(
            parse(&format!("state=done:msg={}", b64(&"a".repeat(2049)))),
            Report::Ignored
        );
        assert_ne!(
            parse(&format!("state=done:msg={}", b64(&"a".repeat(2048)))),
            Report::Ignored
        );
        assert_eq!(
            parse(&format!("state=done:title={}", b64(&"a".repeat(193)))),
            Report::Ignored
        );
        assert_eq!(
            parse(&format!("state=done:app={}", "a".repeat(33))),
            Report::Ignored
        );
        assert_eq!(
            parse(&format!("state=done:id={}", "a".repeat(33))),
            Report::Ignored
        );
        let deep = ["a"; 9].join("/");
        assert_eq!(parse(&format!("state=done:id={deep}")), Report::Ignored);
        assert_eq!(parse("state=done:app=has space"), Report::Ignored);
    }

    #[test]
    fn the_small_rules_follow_the_spec() {
        assert_eq!(
            set("state=idle:state=done").state,
            State::Done,
            "last one wins"
        );
        assert_eq!(
            set("state=done:nonsense:future=1").state,
            State::Done,
            "unknown is skipped"
        );
        assert_eq!(
            set("state=working:kind=permission").kind,
            None,
            "kind is for blocked"
        );
        assert_eq!(parse("clear"), Report::Clear(None));
        assert_eq!(
            parse("clear:id=a/b"),
            Report::Clear(Some("a/b".to_string()))
        );
    }

    #[test]
    fn a_report_replaces_its_record_whole() {
        let mut records = Records::default();
        records.apply(parse(&format!("state=working:msg={}", b64("Tests"))));
        records.apply(parse("state=idle"));
        let top = records.summary().expect("a record");
        assert_eq!(top.state, State::Idle);
        assert_eq!(top.msg, None, "omitted keys are removed");
    }

    #[test]
    fn what_needs_the_person_comes_first() {
        let mut records = Records::default();
        records.apply(parse("state=done:id=a"));
        records.apply(parse("state=working:id=b"));
        assert_eq!(records.summary().map(|r| r.state), Some(State::Working));
        records.apply(parse("state=error:id=c"));
        assert_eq!(records.summary().map(|r| r.state), Some(State::Error));
        records.apply(parse("state=blocked:id=d"));
        assert_eq!(records.summary().map(|r| r.state), Some(State::Blocked));
    }

    #[test]
    fn clearing_takes_children_and_nothing_else() {
        let mut records = Records::default();
        records.apply(parse("state=working:id=a"));
        records.apply(parse("state=working:id=a/b"));
        records.apply(parse("state=done:id=ab"));
        records.apply(parse("clear:id=a"));
        assert_eq!(records.summary().map(|r| r.id.as_str()), Some("ab"));
        records.apply(parse("clear"));
        assert_eq!(records.summary(), None);
    }

    #[test]
    fn a_new_prompt_ends_work_but_keeps_results() {
        // Break caught: a killed agent showing Working forever, or its result
        // vanishing before anyone saw it.
        let mut records = Records::default();
        records.apply(parse("state=working:id=a"));
        records.apply(parse("state=blocked:id=b"));
        records.apply(parse("state=idle:id=c"));
        records.apply(parse("state=done:id=d"));
        records.apply(parse("state=error:id=e"));
        records.program_gone();
        assert_eq!(records.records.len(), 2);
        assert_eq!(records.summary().map(|r| r.state), Some(State::Error));
    }

    #[test]
    fn seeing_a_result_dismisses_it() {
        let mut records = Records::default();
        records.apply(parse("state=done:id=a"));
        records.apply(parse("state=error:id=b"));
        records.apply(parse("state=working:id=c"));
        records.mark_seen();
        assert_eq!(records.records.len(), 1);
        assert_eq!(records.summary().map(|r| r.state), Some(State::Working));
    }

    #[test]
    fn a_flood_of_ids_stays_bounded() {
        let mut records = Records::default();
        for n in 0..1000 {
            records.apply(parse(&format!("state=working:id=a{n}")));
        }
        assert_eq!(records.records.len(), MAX_RECORDS);
        assert_eq!(records.records.first().map(|r| r.id.as_str()), Some("a936"));
    }

    #[test]
    fn the_reply_can_cross_the_windows_console_as_keys() {
        // Break caught: the console dropping the reply, so Claude Code never
        // learned that status reports are welcome.
        assert_eq!(
            String::from_utf8_lossy(&as_console_keys(b"\x1b]")),
            "\x1b[0;0;27;1;0;1_\x1b[0;0;27;0;0;1_\x1b[0;0;93;1;0;1_\x1b[0;0;93;0;0;1_"
        );
        assert_eq!(as_console_keys(b""), Vec::<u8>::new());
    }
}
