//! Diagnostic `file:line:column` recognition (issue #51, #41 slice 2).
//!
//! Compiler and stack-trace references (cargo/clippy, tsc, python, WSL/gcc)
//! are recognized by scanning the clicked line only: nothing is stored on
//! grid cells, so auto-detected text can never take the OSC 8 link paint
//! path (SEC-022). Paths are anchored at word boundaries by pattern shape
//! plus post-match filters (the regex crate has no lookaround); relative
//! paths resolve against the pane's OSC 7 cwd; non-local OSC 7 hosts are
//! copy-path-only (documented, LINK-008).

use std::sync::OnceLock;

use regex::Regex;

/// A recognized reference: path plus 1-based line and optional column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    /// Path text exactly as it appeared in the line.
    pub path: String,
    /// 1-based line number.
    pub line: u32,
    /// 1-based column, when the format carried one.
    pub col: Option<u32>,
}

/// A reference resolved against the pane's context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// A local path an editor can open.
    Open {
        /// Resolved absolute path (Windows form).
        path: String,
        /// 1-based line number.
        line: u32,
        /// 1-based column, when known.
        col: Option<u32>,
    },
    /// Not openable here (remote OSC 7 host, unmappable POSIX path, no
    /// cwd): copy the reference text, never launch anything.
    CopyOnly {
        /// The reference text as it appeared.
        text: String,
    },
}

/// Shared path-shape alternatives: Windows drive, UNC, POSIX absolute,
/// relative with a separator, bare filename with an extension. Paths may
/// contain spaces; the trailing segments are greedy and let the engine
/// backtrack to the `:line` / `(line,col)` suffix. The first segment of
/// the relative branch excludes space via `\x20` (in `(?x)` mode the
/// regex crate strips literal spaces, even inside a class).
const PATH_ALT: &str = r"(?:
      [A-Za-z]:[\\/][^:\r\n]*
    | \\\\[^:\r\n\\/]+\\[^:\r\n\\/]+(?:\\[^:\r\n]*)?
    | /[^\r\n:]*
    | [\p{L}\p{N}][^:\r\n/\\\x20]*[/\\][^:\r\n]*
    | [\p{L}\p{N}][\p{L}\p{N}._+\-]*\.[A-Za-z][A-Za-z0-9]{0,10}
)";

/// `path:line` / `path:line:col` (cargo, clippy, gcc, jest, maven).
fn colon_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r"(?x)({PATH_ALT}):(\d+)(?::(\d+))?"))
            .unwrap_or_else(|e| panic!("static colon regex: {e}"))
    })
}

/// `path(line,col)` / `path(line)` (tsc, maven brackets ride the colon rule).
fn tsc_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r"(?x)({PATH_ALT})\((\d+)(?:,(\d+))?\)"))
            .unwrap_or_else(|e| panic!("static tsc regex: {e}"))
    })
}

/// Python traceback frames: `File "x.py", line 12, column 5`.
fn python_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"File "([^"\r\n]+)", line (\d+)(?:, column (\d+))?"#)
            .unwrap_or_else(|e| panic!("static python regex: {e}"))
    })
}

/// Character that must precede a colon/tsc match for it to count as a
/// path: line start, whitespace, or a quoting/punctuation opener. This is
/// the path-anchoring filter that rejects mid-word junk (`xfile.rs:1:2`).
fn ok_prefix(text: &str, start: usize) -> bool {
    match text[..start].chars().next_back() {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '[' | '(' | '"' | '<' | ',' | '=' | '\''),
    }
}

/// Character immediately after a match must not be alphanumeric
/// (`file.rs:12:34x` is not a reference).
fn ok_suffix(text: &str, end: usize) -> bool {
    !text[end..]
        .chars()
        .next()
        .is_some_and(char::is_alphanumeric)
}

/// One candidate match plus its byte span in the scanned line.
struct Cand {
    start: usize,
    end: usize,
    rf: FileRef,
}

fn make_cand(text: &str, caps: regex::Captures<'_>, is_python: bool) -> Option<Cand> {
    let m = caps.get(0)?;
    let path = caps.get(1)?.as_str();
    if path.contains("://") || !ok_prefix(text, m.start()) {
        return None;
    }
    if !is_python && !ok_suffix(text, m.end()) {
        return None;
    }
    let line = caps.get(2)?.as_str().parse().unwrap_or(0);
    let col = caps.get(3).and_then(|c| c.as_str().parse().ok());
    let (start, end) = if is_python {
        (m.start(), m.end())
    } else {
        (
            text[..m.start()].chars().count(),
            text[..m.end()].chars().count(),
        )
    };
    Some(Cand {
        start,
        end,
        rf: FileRef {
            path: path.to_string(),
            line,
            col,
        },
    })
}

/// Find the reference whose span contains `click_col` (a grid cell column,
/// 1:1 with char index on a grid line). Returns the char range of the
/// whole reference and the parsed reference. Scans all three formats and
/// prefers colon, then tsc, then python for overlapping matches.
pub fn find_ref(line_text: &str, click_col: usize) -> Option<(std::ops::Range<usize>, FileRef)> {
    let mut cands: Vec<Cand> = Vec::new();
    for c in colon_re().captures_iter(line_text) {
        if let Some(c) = make_cand(line_text, c, false) {
            cands.push(c);
        }
    }
    for c in tsc_re().captures_iter(line_text) {
        if let Some(c) = make_cand(line_text, c, false) {
            cands.push(c);
        }
    }
    for c in python_re().captures_iter(line_text) {
        if let Some(c) = make_cand(line_text, c, true) {
            cands.push(c);
        }
    }
    cands
        .into_iter()
        .find(|c| c.start <= click_col && click_col < c.end)
        .map(|c| (c.start..c.end, c.rf))
}

/// Strip a leading slash from an OSC 7 path that carries a drive letter
/// (`/C:/src/rust` -> `C:/src/rust`).
fn normalize_cwd(cwd: &str) -> String {
    let b = cwd.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        cwd[1..].to_string()
    } else {
        cwd.to_string()
    }
}

/// True for `/mnt/<drive>/...` (WSL view of a Windows volume).
fn wsl_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/mnt/")?;
    let (drive, tail) = rest.split_once('/')?;
    let drive = drive.as_bytes();
    if drive.len() == 1 && drive[0].is_ascii_alphabetic() {
        Some(format!(
            "{}:\\{}",
            drive[0].to_ascii_uppercase() as char,
            tail
        ))
    } else {
        None
    }
}

/// Resolve a reference against the pane context. A remote OSC 7 host
/// (`remote`) is copy-path-only; `/mnt/<d>/` maps to `<D>:\`; Windows
/// absolute paths pass through; relative paths join the pane cwd (copy-only
/// when there is no cwd); other POSIX absolute paths are copy-only.
pub fn resolve(rf: &FileRef, cwd: Option<&str>, remote: bool) -> Resolved {
    let text = match rf.col {
        Some(c) => format!("{}:{}:{}", rf.path, rf.line, c),
        None => format!("{}:{}", rf.path, rf.line),
    };
    if remote {
        return Resolved::CopyOnly { text };
    }
    let b = rf.path.as_bytes();
    let windows_abs = (b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && matches!(b[2], b'/' | b'\\'))
        || rf.path.starts_with("\\\\");
    if windows_abs {
        return Resolved::Open {
            path: rf.path.replace('/', "\\"),
            line: rf.line,
            col: rf.col,
        };
    }
    if rf.path.starts_with('/') {
        return match wsl_path(&rf.path) {
            Some(p) => Resolved::Open {
                path: p.replace('/', "\\"),
                line: rf.line,
                col: rf.col,
            },
            None => Resolved::CopyOnly { text },
        };
    }
    match cwd {
        Some(cwd) => Resolved::Open {
            path: format!("{}\\{}", normalize_cwd(cwd), rf.path).replace('/', "\\"),
            line: rf.line,
            col: rf.col,
        },
        None => Resolved::CopyOnly { text },
    }
}

/// Build the editor argv from the config template. Placeholders `{file}`,
/// `{line}`, `{column}` are substituted per whitespace-separated token; a
/// template with no `{file}` gets `path:line:col` appended (the
/// `code --goto "editor = "code --goto""` convention). A missing column
/// drops `{column}` and a trailing colon. An empty template is disabled
/// (`None`).
pub fn editor_command(
    template: &str,
    path: &str,
    line: u32,
    col: Option<u32>,
) -> Option<(String, Vec<String>)> {
    let template = template.trim();
    if template.is_empty() {
        return None;
    }
    let col_text = col.map_or(String::new(), |c| c.to_string());
    let mut tokens: Vec<String> = template
        .split_whitespace()
        .map(|t| {
            t.replace("{file}", path)
                .replace("{line}", &line.to_string())
                .replace("{column}", &col_text)
        })
        .collect();
    if col.is_none() {
        for t in &mut tokens {
            *t = t.trim_end_matches(':').to_string();
        }
        tokens.retain(|t| !t.is_empty());
    }
    if !tokens.iter().any(|t| t.contains(path)) && !template.contains("{file}") {
        let suffix = match col {
            Some(c) => format!("{path}:{line}:{c}"),
            None => format!("{path}:{line}"),
        };
        tokens.push(suffix);
    }
    if tokens.is_empty() {
        return None;
    }
    let program = tokens.remove(0);
    Some((program, tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, col: usize) -> FileRef {
        find_ref(text, col).map(|(_, r)| r).expect("no ref")
    }

    #[test]
    fn static_regexes_compile() {
        // The unwrap_or_else(panic) sites above are unreachable while this
        // passes; the regex crate validates all three patterns here.
        assert!(colon_re().is_match("a/b.rs:1:2"));
        assert!(tsc_re().is_match("a/b.rs(1,2)"));
        assert!(python_re().is_match("File \"a.py\", line 1"));
    }

    #[test]
    fn drive_path_with_spaces() {
        let r = at(
            r"error[E0308]: C:\Users\dhruv\my file.rs:12:5: expected T",
            20,
        );
        assert_eq!(r.path, r"C:\Users\dhruv\my file.rs");
        assert_eq!(r.line, 12);
        assert_eq!(r.col, Some(5));
    }

    #[test]
    fn relative_and_bare_and_unicode() {
        assert_eq!(at("  --> src/main.rs:3:5", 8).path, "src/main.rs");
        assert_eq!(at("warning: Cargo.toml:5:1: unused", 10).path, "Cargo.toml");
        assert_eq!(at("data данные.rs:7:2 boom", 6).path, "данные.rs");
    }

    #[test]
    fn unc_path() {
        let r = at(r"\\server\share\my file.rs:9:1: err", 5);
        assert_eq!(r.path, r"\\server\share\my file.rs");
        assert_eq!(r.line, 9);
    }

    #[test]
    fn tsc_and_python_styles() {
        let r = at("src/app ts.ts(12,5): error TS2304: x", 4);
        assert_eq!(r.path, "src/app ts.ts");
        assert_eq!((r.line, r.col), (12, Some(5)));
        let r = at("  File \"/mnt/c/proj/x.py\", line 42, column 7", 8);
        assert_eq!(r.path, "/mnt/c/proj/x.py");
        assert_eq!((r.line, r.col), (42, Some(7)));
        let r = at("  File \"a.py\", line 3", 4);
        assert_eq!((r.line, r.col), (3, None));
    }

    #[test]
    fn click_position_selects_the_right_ref() {
        let text = "see src/a.rs:1:2 and src/b.rs:3:4 now";
        let ca = text.find("a.rs").unwrap();
        let cb = text.find("b.rs").unwrap();
        assert_eq!(find_ref(text, ca).unwrap().1.path, "src/a.rs");
        assert_eq!(find_ref(text, cb).unwrap().1.path, "src/b.rs");
    }

    #[test]
    fn junk_is_not_a_reference() {
        // mid-word, url, time, host:port
        assert!(find_ref("data:file.rs:1:2", 6).is_none());
        assert!(find_ref("see http://host:8080/path", 6).is_none());
        assert!(find_ref("note: 12:34 today", 7).is_none());
        assert!(find_ref("192.168.0.1:8080", 3).is_none());
        assert!(find_ref("file.rs:12:34x tail", 2).is_none());
    }

    #[test]
    fn resolve_relative_against_cwd() {
        let rf = FileRef {
            path: "src/a.rs".into(),
            line: 1,
            col: None,
        };
        let r = resolve(&rf, Some("/C:/src/rust"), false);
        assert!(matches!(r, Resolved::Open{ path, .. } if path == r"C:\src\rust\src\a.rs"));
        let r = resolve(&rf, None, false);
        assert!(matches!(r, Resolved::CopyOnly { .. }));
    }

    #[test]
    fn resolve_wsl_and_posix_and_remote() {
        let rf = FileRef {
            path: "/mnt/c/dev/x.c".into(),
            line: 4,
            col: Some(1),
        };
        let r = resolve(&rf, None, false);
        assert!(matches!(r, Resolved::Open{ path, .. } if path == r"C:\dev\x.c"));
        let rf = FileRef {
            path: "/usr/include/x.h".into(),
            line: 4,
            col: None,
        };
        assert!(matches!(
            resolve(&rf, None, false),
            Resolved::CopyOnly { .. }
        ));
        let rf = FileRef {
            path: "src/a.rs".into(),
            line: 1,
            col: Some(2),
        };
        let r = resolve(&rf, Some("/tmp"), true);
        assert!(matches!(r, Resolved::CopyOnly { text } if text == "src/a.rs:1:2"));
    }

    #[test]
    fn editor_argv() {
        let (p, a) = editor_command(
            "code --goto {file}:{line}:{column}",
            r"C:\p a\x.rs",
            12,
            Some(5),
        )
        .unwrap();
        assert_eq!(p, "code");
        assert_eq!(
            a,
            vec!["--goto".to_string(), r"C:\p a\x.rs:12:5".to_string()]
        );
        // missing column: trailing colon dropped
        let (p, a) = editor_command("nvim {file}+{line}", "x.rs", 7, None).unwrap();
        assert_eq!(p, "nvim");
        assert_eq!(a, vec!["x.rs+7".to_string()]);
        // bare template appends path:line:col
        let (_p, a) = editor_command("code --goto", "x.rs", 3, Some(1)).unwrap();
        assert_eq!(a, vec!["--goto".to_string(), "x.rs:3:1".to_string()]);
        assert!(editor_command("", "x.rs", 1, None).is_none());
    }

    #[test]
    fn huge_line_is_linear_and_fails_safe() {
        let line = format!("{} src/a.rs:1:1", "x".repeat(100_000));
        let col = line.find("src").unwrap();
        assert_eq!(find_ref(&line, col).unwrap().1.path, "src/a.rs");
        let junk = "x".repeat(100_000);
        assert!(find_ref(&junk, 5).is_none());
    }
}
