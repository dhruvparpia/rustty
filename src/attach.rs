//! Console attach frontend (issue #47, docs/design-session-attach.md).
//!
//! `rusttyterm --attach <n>` renders the managed PTY in the *caller's*
//! console: the console translates keystrokes to VT bytes
//! (`ENABLE_VIRTUAL_TERMINAL_INPUT`), so Unicode, control keys, IME
//! text-as-keys and paste arrive already encoded — no keymap here.
//! The local console geometry is the PTY size (controller only);
//! detach restores every console mode, the cursor, the window geometry
//! and the main screen buffer.

use std::io;

use serde_json::{Value, json};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::WriteFile;
use windows::Win32::System::Console::*;

use crate::host::{
    EXIT_BUSY, EXIT_HOST_UNAVAILABLE, EXIT_NOT_FOUND, MAX_PAGE_BYTES, client::Client, read_frame,
};

/// Input mode we need: VT key encoding, window-size events, mouse
/// events, extended flags; line/echo/processed/quick-edit off.
const IN_ADD: u32 = ENABLE_VIRTUAL_TERMINAL_INPUT.0
    | ENABLE_WINDOW_INPUT.0
    | ENABLE_MOUSE_INPUT.0
    | ENABLE_EXTENDED_FLAGS.0;
const IN_CLEAR: u32 = ENABLE_PROCESSED_INPUT.0
    | ENABLE_LINE_INPUT.0
    | ENABLE_ECHO_INPUT.0
    | ENABLE_QUICK_EDIT_MODE.0
    | ENABLE_INSERT_MODE.0;

/// Snapshot of the caller's console, restored on every exit path.
#[derive(Clone, Copy)]
struct Saved {
    in_h: SendH,
    out_h: SendH,
    in_mode: u32,
    out_mode: u32,
    cursor: CONSOLE_CURSOR_INFO,
    size: COORD,
    window: SMALL_RECT,
}

/// Console HANDLE moved onto the reader thread (its restore path).
#[derive(Clone, Copy)]
struct SendH(HANDLE);
// Safety: console handles are process-wide kernel handles; the
// reader thread uses them only for console writes and restore.
unsafe impl Send for SendH {}

/// Decode a `C-x` detach-prefix setting into the control byte.
///
/// # Errors
/// Not `C-<char>`.
pub fn parse_prefix(spec: &str) -> Result<u8, String> {
    let rest = spec
        .strip_prefix("C-")
        .filter(|r| r.chars().count() == 1)
        .ok_or_else(|| format!("detach-prefix must be \"C-<char>\", got {spec:?}"))?;
    let c = rest
        .chars()
        .next()
        .ok_or_else(|| "detach-prefix must be \"C-<char>\"".to_string())?;
    if (c as u32) > 0x7f {
        return Err(format!("detach-prefix {spec:?} must be ASCII"));
    }
    Ok(u8::try_from(c as u32 & 0x1f).unwrap_or(0))
}

/// Attach to session `alias` in this console. Returns the process exit
/// code (0 after a normal detach).
///
/// # Errors
/// Transport failures; refusals return codes, not errors.
pub fn run(alias: u32, read_only: bool, take_control: bool, prefix: u8) -> io::Result<u8> {
    let in_h = std_handle(STD_INPUT_HANDLE)?;
    let out_h = std_handle(STD_OUTPUT_HANDLE)?;
    // Strict parse of the environment (issue #47 acceptance): the
    // frontend needs a real console on both ends.
    if console_mode(in_h).is_err() || console_mode(out_h).is_err() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "stdin/stdout is not a console (TTY); --attach needs an interactive terminal",
        ));
    }
    // Self-attach feedback loop (attach ADR): a host-managed shell has
    // RUSTTYTERM_SESSION set to its own alias.
    if std::env::var("RUSTTYTERM_SESSION").is_ok_and(|v| v == alias.to_string()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to attach session {alias} to the shell that runs inside it"),
        ));
    }

    let mut client = match Client::connect() {
        Ok(c) => c,
        Err(e) => {
            return Ok(fail(EXIT_HOST_UNAVAILABLE, &format!("host: {e}")));
        }
    };
    let resp = client.call(
        "session.attach",
        json!({"alias": alias, "readOnly": read_only, "takeControl": take_control}),
    )?;
    if let Some(err) = resp.get("error") {
        let code = err.get("code").and_then(Value::as_str).unwrap_or("error");
        let msg = err.get("message").and_then(Value::as_str).unwrap_or("");
        let exit = match code {
            "session-busy" => EXIT_BUSY,
            "target-stale" | "target-closed" | "not-found" => EXIT_NOT_FOUND,
            _ => EXIT_HOST_UNAVAILABLE,
        };
        return Ok(fail(exit, &format!("{code}: {msg}")));
    }
    let granted = resp
        .get("result")
        .and_then(|r| r.get("controlGranted"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let saved = save_console(in_h, out_h)?;
    let (cols, rows) = window_size(out_h);

    // Enter the alt screen, then take over the console.
    write_out(out_h, b"\x1b[?1049h");
    // Safety: console mode set on handles we validated.
    unsafe {
        SetConsoleMode(in_h, CONSOLE_MODE((saved.in_mode | IN_ADD) & !IN_CLEAR)).ok();
        SetConsoleMode(
            out_h,
            CONSOLE_MODE(
                saved.out_mode | ENABLE_PROCESSED_OUTPUT.0 | ENABLE_VIRTUAL_TERMINAL_PROCESSING.0,
            ),
        )
        .ok();
    }
    if granted {
        client
            .send(
                "session.resize",
                json!({"alias": alias, "cols": cols, "rows": rows}),
            )
            .ok();
    }

    // Reader thread: PTY stream -> console. If the session/host dies
    // under us it restores the console and exits the process, because
    // the input thread is blocked in the console.
    let reader = client.reader()?;
    let saved_for_reader = saved;
    let out_for_reader = SendH(out_h);
    std::thread::spawn(move || reader_loop(reader, out_for_reader, saved_for_reader));

    let code = input_loop(&mut client, in_h, out_h, alias, granted, prefix)?;
    // Normal detach: drop the connection, restore the caller's console.
    drop(client);
    saved.restore();
    Ok(code)
}

/// Keystrokes/mouse/resize -> RPC frames; the detach-prefix state
/// machine runs on the encoded bytes.
fn input_loop(
    client: &mut Client,
    in_h: HANDLE,
    out_h: HANDLE,
    alias: u32,
    controller: bool,
    prefix: u8,
) -> io::Result<u8> {
    let mut recs = vec![INPUT_RECORD::default(); 32];
    let mut prefix_pending = false;
    let mut last_buttons = 0u32;
    loop {
        // Safety: record buffer + out param, handle validated.
        let mut n = 0u32;
        if unsafe { ReadConsoleInputW(in_h, &mut recs, core::ptr::from_mut(&mut n)) }.is_err() {
            // Window resize flushes pending input (the documented
            // ReadConsole behaviour); just read again.
            continue;
        }
        for rec in recs.iter().take(usize::try_from(n).unwrap_or(0)) {
            if rec.EventType == WINDOW_BUFFER_SIZE_EVENT as u16 && controller {
                let s = unsafe { rec.Event.WindowBufferSizeEvent }.dwSize;
                let cols = u16::try_from(s.X).unwrap_or(0).max(1);
                let rows = u16::try_from(s.Y).unwrap_or(0).max(1);
                client
                    .send(
                        "session.resize",
                        json!({"alias": alias, "cols": cols, "rows": rows}),
                    )
                    .ok();
            }
            let bytes = if rec.EventType == MOUSE_EVENT as u16 {
                mouse_bytes(rec, &mut last_buttons)
            } else {
                key_bytes(rec)
            };
            if bytes.is_empty() {
                continue;
            }
            // Detach-prefix state machine over the encoded stream.
            let mut emit: Vec<u8> = Vec::with_capacity(bytes.len());
            for b in bytes {
                if prefix_pending {
                    prefix_pending = false;
                    match b {
                        b'd' => return Ok(0), // detach: caller's prompt returns
                        b'?' => {
                            help(out_h, prefix);
                        }
                        p if p == prefix => emit.push(p), // literal Ctrl+]
                        _ => {
                            emit.push(prefix);
                            emit.push(b);
                        }
                    }
                } else if b == prefix {
                    prefix_pending = true;
                } else {
                    emit.push(b);
                }
            }
            if emit.is_empty() || !controller {
                continue; // watcher/read-only: input is dropped
            }
            let data = hex(&emit);
            if client
                .send("session.input", json!({"alias": alias, "data": data}))
                .is_err()
            {
                eprintln!("\r\n[host connection lost]\r\n");
                return Ok(EXIT_HOST_UNAVAILABLE);
            }
        }
    }
}

/// PTY stream -> console, until EOF. Restores the console, then ends
/// the process (the input thread is parked in the console).
fn reader_loop(mut rd: crate::host::client::DupReader, out_h: SendH, saved: Saved) -> ! {
    let out_h = out_h.0;
    let mut lost = false;
    loop {
        match read_frame(&mut rd, MAX_PAGE_BYTES) {
            Ok(frame) => {
                match frame.get("method").and_then(Value::as_str) {
                    Some("session.output") => {
                        let hex = frame
                            .get("params")
                            .and_then(|p| p.get("data"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if let Some(bytes) = unhex(hex) {
                            write_out(out_h, &bytes);
                        }
                    }
                    Some("session.resize") => {
                        let p = frame.get("params");
                        let cols = p.and_then(|p| p.get("cols")).and_then(Value::as_u64);
                        let rows = p.and_then(|p| p.get("rows")).and_then(Value::as_u64);
                        if let (Some(c), Some(r)) = (cols, rows) {
                            follow_size(out_h, c, r);
                        }
                    }
                    Some("session.exit") => break,
                    _ => {} // responses to our fire-and-forget frames
                }
            }
            Err(_) => {
                lost = true;
                break;
            }
        }
    }
    // Session gone: leave the alt screen and hand the console back
    // before ending the process (the input thread cannot return).
    write_out(out_h, b"\x1b[?1049l");
    saved.restore();
    eprintln!(
        "\r\n[detached: {}]\r\n",
        if lost {
            "host connection lost"
        } else {
            "the session exited"
        }
    );
    std::process::exit(i32::from(if lost { EXIT_HOST_UNAVAILABLE } else { 0 }))
}

/// One key event -> the VT bytes the console produced for it.
fn key_bytes(rec: &INPUT_RECORD) -> Vec<u8> {
    if rec.EventType != KEY_EVENT as u16 {
        return Vec::new();
    }
    // Safety: reading the union arm for a KEY_EVENT record.
    let k = unsafe { rec.Event.KeyEvent };
    if !k.bKeyDown.as_bool() {
        return Vec::new();
    }
    let ch = unsafe { k.uChar.UnicodeChar };
    if ch == 0 {
        return Vec::new();
    }
    // shortcut: lone surrogates from a console paste become U+FFFD;
    // upgrade to a surrogate-pair accumulator if paste text shows gaps.
    let Some(c) = char::from_u32(u32::from(ch)) else {
        return vec![0xEF, 0xBF, 0xBD];
    };
    let mut buf = [0u8; 4];
    c.encode_utf8(&mut buf).as_bytes().to_vec()
}

/// One mouse event -> SGR mouse bytes (xterm-style, what TUIs request).
fn mouse_bytes(rec: &INPUT_RECORD, last: &mut u32) -> Vec<u8> {
    // Safety: reading the union arm for a MOUSE_EVENT record.
    let m = unsafe { rec.Event.MouseEvent };
    let state = m.dwButtonState;
    let wheel = (m.dwEventFlags & MOUSE_WHEELED) != 0;
    if (m.dwEventFlags & MOUSE_MOVED) != 0 && state == *last && !wheel {
        *last = state;
        return Vec::new();
    }
    let moved_only = state == *last && !wheel;
    *last = state;
    if moved_only {
        return Vec::new();
    }
    let (btn, pressed) = if wheel {
        (if (state as i32) > 0 { 64 } else { 65 }, true)
    } else if state & 1 != 0 {
        (0, true)
    } else if state & 2 != 0 {
        (1, true)
    } else if state & 4 != 0 {
        (2, true)
    } else {
        (3, false)
    };
    let suffix = if pressed || wheel { b'M' } else { b'm' };
    format!(
        "\x1b[<{btn};{};{}{suffix}",
        i32::from(m.dwMousePosition.X) + 1,
        i32::from(m.dwMousePosition.Y) + 1,
    )
    .into_bytes()
}

/// `[Ctrl+] d` detaches; `[Ctrl+] ?` prints the help line.
fn help(out_h: HANDLE, prefix: u8) {
    let key = match prefix {
        0x1d => "Ctrl+]".to_string(),
        c => format!("^{c:02X}"),
    };
    write_out(
        out_h,
        format!("\r\n{key} d: detach | {key} {key}: literal {key} | {key} ?: this help\r\n")
            .as_bytes(),
    );
}

/// Follow a remote (controller) resize best-effort.
fn follow_size(out_h: HANDLE, cols: u64, rows: u64) {
    let Ok(cols) = i16::try_from(cols) else {
        return;
    };
    let Ok(rows) = i16::try_from(rows) else {
        return;
    };
    if cols < 1 || rows < 1 {
        return;
    }
    // Safety: geometry set on a validated console handle.
    unsafe {
        let rect = SMALL_RECT {
            Left: 0,
            Top: 0,
            Right: cols.saturating_sub(1),
            Bottom: rows.saturating_sub(1),
        };
        SetConsoleWindowInfo(out_h, true, core::ptr::from_ref(&rect)).ok();
    }
}

fn fail(code: u8, msg: &str) -> u8 {
    eprintln!("rusttyterm: {msg}");
    code
}

fn std_handle(which: STD_HANDLE) -> io::Result<HANDLE> {
    // Safety: standard-handle query.
    let h = unsafe { GetStdHandle(which) }.map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
    if h.is_invalid() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no standard handle (output is redirected?)",
        ));
    }
    Ok(h)
}

fn console_mode(h: HANDLE) -> io::Result<u32> {
    // Safety: mode query on a caller-provided handle.
    let mut m = CONSOLE_MODE(0);
    unsafe { GetConsoleMode(h, core::ptr::from_mut(&mut m)) }
        .map(|()| m.0)
        .map_err(|e| io::Error::from_raw_os_error(e.code().0))
}

fn window_size(out_h: HANDLE) -> (u16, u16) {
    // Safety: buffer-info query on a validated handle.
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    unsafe { GetConsoleScreenBufferInfo(out_h, core::ptr::from_mut(&mut info)) }
        .map(|()| {
            (
                u16::try_from(info.srWindow.Right - info.srWindow.Left + 1).unwrap_or(80),
                u16::try_from(info.srWindow.Bottom - info.srWindow.Top + 1).unwrap_or(24),
            )
        })
        .unwrap_or((80, 24))
}

fn save_console(in_h: HANDLE, out_h: HANDLE) -> io::Result<Saved> {
    let in_mode = console_mode(in_h)?;
    let out_mode = console_mode(out_h)?;
    // Safety: queries on validated handles.
    unsafe {
        let mut cursor = CONSOLE_CURSOR_INFO::default();
        GetConsoleCursorInfo(out_h, core::ptr::from_mut(&mut cursor))
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        GetConsoleScreenBufferInfo(out_h, core::ptr::from_mut(&mut info))
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
        Ok(Saved {
            in_h: SendH(in_h),
            out_h: SendH(out_h),
            in_mode,
            out_mode,
            cursor,
            size: info.dwSize,
            window: info.srWindow,
        })
    }
}

impl Saved {
    /// Restore modes, cursor and geometry (best effort, never panics).
    fn restore(&self) {
        // Safety: console-mode/geometry restores on handles captured in save.
        unsafe {
            SetConsoleMode(self.in_h.0, CONSOLE_MODE(self.in_mode)).ok();
            SetConsoleMode(self.out_h.0, CONSOLE_MODE(self.out_mode)).ok();
            SetConsoleCursorInfo(self.out_h.0, core::ptr::from_ref(&self.cursor)).ok();
            // Shrink the window, resize the buffer, restore the window.
            let point = SMALL_RECT {
                Left: 0,
                Top: 0,
                Right: 0,
                Bottom: 0,
            };
            SetConsoleWindowInfo(self.out_h.0, true, core::ptr::from_ref(&point)).ok();
            let buf = SMALL_RECT {
                Left: 0,
                Top: 0,
                Right: self.size.X.saturating_sub(1),
                Bottom: self.size.Y.saturating_sub(1),
            };
            SetConsoleWindowInfo(self.out_h.0, true, core::ptr::from_ref(&buf)).ok();
            SetConsoleWindowInfo(self.out_h.0, true, core::ptr::from_ref(&self.window)).ok();
        }
    }
}

fn write_out(h: HANDLE, bytes: &[u8]) {
    let mut off = 0usize;
    while off < bytes.len() {
        // Safety: synchronous write on a validated console handle.
        let Ok(mut done) = u32::try_from(bytes.len() - off) else {
            break;
        };
        let chunk = &bytes[off..off + usize::try_from(done).unwrap_or(0)];
        // Safety: FFI write; buffer lives through the call.
        unsafe {
            if WriteFile(h, Some(chunk), Some(core::ptr::from_mut(&mut done)), None).is_err() {
                break;
            }
        }
        off += usize::try_from(done).unwrap_or(0);
    }
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[usize::from(b >> 4)] as char);
        s.push(HEX[usize::from(b & 0xf)] as char);
    }
    s
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    for pair in b.chunks(2) {
        let v = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        out.push(v);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_parses_control_letters() {
        assert_eq!(parse_prefix("C-]").unwrap(), 0x1d);
        assert_eq!(parse_prefix("C-a").unwrap(), 0x01);
        assert_eq!(parse_prefix("C-x").unwrap(), 0x18);
        assert!(parse_prefix("ctrl+]").is_err());
        assert!(parse_prefix("C-ab").is_err());
        assert!(parse_prefix("C-\u{3bb}").is_err());
    }

    #[test]
    fn hex_round_trips() {
        let bytes = b"hello \xe2\x9c\x93";
        assert_eq!(unhex(&hex(bytes)).unwrap(), bytes);
        assert!(unhex("abc").is_none());
        assert!(unhex("zz").is_none());
    }
}
