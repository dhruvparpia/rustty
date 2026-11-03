//! Console attach frontend (issue #47, docs/design-session-attach.md).
//!
//! `rustty --attach <n>` renders the managed PTY in the *caller's*
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
    // RUSTTY_SESSION set to its own alias.
    if std::env::var("RUSTTY_SESSION").is_ok_and(|v| v == alias.to_string()) {
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

    // The attach result carries the model snapshot (host-snapshot/v1)
    // at a revision: render it, then live frames continue from there.
    let result = resp.get("result");
    if let Some(snap) = result
        .and_then(|r| r.get("snapshot"))
        .filter(|v| v.is_object())
    {
        write_out(out_h, &snapshot_ansi(snap));
    }
    if let Some(deltas) = result
        .and_then(|r| r.get("deltas"))
        .and_then(Value::as_array)
    {
        for d in deltas {
            let hex = d.get(1).and_then(Value::as_str).unwrap_or("");
            if let Some(bytes) = unhex(hex) {
                write_out(out_h, &bytes);
            }
        }
    }
    let initial_rev = result
        .and_then(|r| r.get("revision"))
        .and_then(Value::as_u64)
        .unwrap_or(0);

    // Reader thread: PTY stream -> console. If the session/host dies
    // under us it restores the console and exits the process, because
    // the input thread is blocked in the console.
    let reader = client.reader()?;
    let writer = client.writer()?;
    let saved_for_reader = saved;
    let out_for_reader = SendH(out_h);
    // Shared control flag: the reader sees a takeover and stops the
    // input loop from sending (issue #49).
    let control = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(granted));
    let control_for_reader = std::sync::Arc::clone(&control);
    std::thread::spawn(move || {
        reader_loop(
            reader,
            writer,
            out_for_reader,
            saved_for_reader,
            alias,
            initial_rev,
            control_for_reader,
        );
    });

    let code = input_loop(&mut client, in_h, out_h, alias, &control, prefix)?;
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
    control: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    prefix: u8,
) -> io::Result<u8> {
    use std::sync::atomic::Ordering;
    let mut recs = vec![INPUT_RECORD::default(); 32];
    let mut prefix_pending = false;
    let mut last_buttons = 0u32;
    let mut demoted_shown = false;
    loop {
        // Safety: record buffer + out param, handle validated.
        let mut n = 0u32;
        if unsafe { ReadConsoleInputW(in_h, &mut recs, core::ptr::from_mut(&mut n)) }.is_err() {
            // Window resize flushes pending input (the documented
            // ReadConsole behaviour); just read again.
            continue;
        }
        for rec in recs.iter().take(usize::try_from(n).unwrap_or(0)) {
            let controller = control.load(Ordering::Relaxed);
            if !controller && !demoted_shown {
                demoted_shown = true;
                write_out(out_h, b"\x1b[2J\x1b[H[you no longer control this session - detach with Ctrl+] d and re-attach with --take-control]\r\n");
            }
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

/// PTY stream -> console, until EOF. Frames carry the host's model
/// revision; a gap (this client was a saturated watcher) is
/// resynchronized through a fresh snapshot over the same connection:
/// live bytes are buffered during the resync so a query is answered
/// exactly once. Restores the console at EOF, then ends the process
/// (the input thread is parked in the console).
#[allow(clippy::too_many_lines, reason = "the reader is one state machine")]
fn reader_loop(
    mut rd: crate::host::client::DupReader,
    mut wr: crate::host::client::DupWriter,
    out_h: SendH,
    saved: Saved,
    alias: u32,
    initial_rev: u64,
    control: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> ! {
    let out_h = out_h.0;
    let mut last_rev = initial_rev;
    let mut resyncing = false;
    let mut pending: Vec<Value> = Vec::new();
    let mut lost = false;
    loop {
        match read_frame(&mut rd, MAX_PAGE_BYTES) {
            Ok(frame) => {
                let method = frame.get("method").and_then(Value::as_str);
                let params = frame.get("params");
                let rev = params
                    .and_then(|p| p.get("revision"))
                    .and_then(Value::as_u64);
                match method {
                    Some("session.output") | Some("session.resize") => {
                        if resyncing {
                            pending.push(frame);
                            continue;
                        }
                        let Some(rev) = rev else { continue };
                        if rev <= last_rev {
                            continue;
                        }
                        if rev > last_rev + 1 {
                            pending.push(frame);
                            request_resync(&mut wr, alias, Some(last_rev));
                            resyncing = true;
                            continue;
                        }
                        last_rev = rev;
                        if method == Some("session.output") {
                            let hex = params
                                .and_then(|p| p.get("data"))
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            if let Some(bytes) = unhex(hex) {
                                write_out(out_h, &bytes);
                            }
                        } else {
                            let cols = params.and_then(|p| p.get("cols")).and_then(Value::as_u64);
                            let rows = params.and_then(|p| p.get("rows")).and_then(Value::as_u64);
                            if let (Some(c), Some(r)) = (cols, rows) {
                                follow_size(out_h, c, r);
                            }
                        }
                    }
                    Some("session.exit") => break,
                    Some("session.control") => {
                        // A takeover demotes us: stop the input loop
                        // from sending (issue #49).
                        if params
                            .and_then(|p| p.get("controller"))
                            .and_then(Value::as_bool)
                            == Some(false)
                        {
                            control.store(false, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    Some("session.resync") if !resyncing => {
                        request_resync(&mut wr, alias, None);
                        resyncing = true;
                    }
                    None if resyncing
                        && frame.get("id").and_then(Value::as_str) == Some("resync") =>
                    {
                        resyncing = false;
                        let result = frame.get("result");
                        let mut installed = last_rev;
                        if let Some(snap) = result
                            .and_then(|r| r.get("snapshot"))
                            .filter(|v| v.is_object())
                        {
                            write_out(out_h, &snapshot_ansi(snap));
                            installed = snap.get("revision").and_then(Value::as_u64).unwrap_or(0);
                        } else if let Some(r) = result
                            .and_then(|r| r.get("revision"))
                            .and_then(Value::as_u64)
                        {
                            installed = r;
                        }
                        last_rev = installed;
                        let frames = std::mem::take(&mut pending);
                        for (idx, f) in frames.iter().enumerate() {
                            let p = f.get("params");
                            let Some(r) = p.and_then(|p| p.get("revision")).and_then(Value::as_u64)
                            else {
                                continue;
                            };

                            if r <= last_rev {
                                continue;
                            }
                            if r > last_rev + 1 {
                                for g in &frames[idx..] {
                                    pending.push(g.clone());
                                }
                                request_resync(&mut wr, alias, Some(last_rev));
                                resyncing = true;
                                break;
                            }
                            last_rev = r;
                            let m = f.get("method").and_then(Value::as_str);
                            if m == Some("session.output") {
                                let hex = f
                                    .get("params")
                                    .and_then(|p| p.get("data"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("");
                                if let Some(bytes) = unhex(hex) {
                                    write_out(out_h, &bytes);
                                }
                            } else if m == Some("session.resize") {
                                let c = f
                                    .get("params")
                                    .and_then(|p| p.get("cols"))
                                    .and_then(Value::as_u64);
                                let r2 = f
                                    .get("params")
                                    .and_then(|p| p.get("rows"))
                                    .and_then(Value::as_u64);
                                if let (Some(c2), Some(r2)) = (c, r2) {
                                    follow_size(out_h, c2, r2);
                                }
                            }
                        }
                    }
                    _ => {}
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

/// Ask the host (on our write handle) for a fresh snapshot attach on
/// this connection; the answer arrives here as the id-matched response.
fn request_resync(wr: &mut crate::host::client::DupWriter, alias: u32, from: Option<u64>) {
    let mut params = serde_json::json!({ "alias": alias, "readOnly": true });
    if let Some(r) = from {
        params["fromRevision"] = serde_json::json!(r);
    }
    let req = serde_json::json!({
        "jsonrpc": "2.0", "id": "resync", "method": "session.attach",
        "params": params,
    });
    let _ = crate::host::write_frame(wr, &req);
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
    eprintln!("rustty: {msg}");
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

/// Render a `host-snapshot/v1` (issue #48) as SGR-restoring bytes: a
/// fresh console running these bytes lands on the same screen the
/// host's model describes, so the live stream after the snapshot
/// revision continues the same state.
pub(crate) fn snapshot_ansi(snap: &Value) -> Vec<u8> {
    let n = |v: Option<&Value>, d: u64| v.and_then(Value::as_u64).unwrap_or(d);
    let rows = usize::try_from(n(snap.get("rows"), 0)).unwrap_or(0);
    let in_alt = snap.get("inAlt").and_then(Value::as_bool).unwrap_or(false);
    let mut out = String::from("\x1b[H\x1b[2J");

    if let Some(modes) = snap.get("modes") {
        let on = |k: &str| modes.get(k).and_then(Value::as_bool).unwrap_or(false);
        out.push_str(&format!(
            "\x1b[?7{}",
            if on("autoWrap") { "h" } else { "l" }
        ));
        out.push_str(&format!(
            "\x1b[?2004{}",
            if on("bracketedPaste") { "h" } else { "l" }
        ));
        out.push_str(&format!(
            "\x1b[?1{}",
            if on("appCursorKeys") { "h" } else { "l" }
        ));
        let mouse = modes.get("mouse").and_then(Value::as_str).unwrap_or("off");
        out.push_str(match mouse {
            "click" => "\x1b[?1000h\x1b[?1002l\x1b[?1003l",
            "drag" => "\x1b[?1002h\x1b[?1000l\x1b[?1003l",
            "any" => "\x1b[?1003h\x1b[?1000l\x1b[?1002l",
            _ => "\x1b[?1000l\x1b[?1002l\x1b[?1003l",
        });
        out.push_str(&format!(
            "\x1b[?1006{}",
            if on("mouseSgr") { "h" } else { "l" }
        ));
        out.push_str(&format!(
            "\x1b[?1015{}",
            if on("mouseUrxvt") { "h" } else { "l" }
        ));
    }

    if in_alt {
        let main_rows = usize::try_from(n(snap.get("mainRows"), 0)).unwrap_or(0);
        paint_grid(&mut out, snap.get("mainLines"), main_rows);
        out.push_str("\x1b[?1049h");
        out.push_str("\x1b[H\x1b[2J");
    }
    paint_grid(&mut out, snap.get("lines"), rows);

    if let Some(pen) = snap.get("pen") {
        let fg = pen.get("fg").and_then(parse_hex_color);
        let bg = pen.get("bg").and_then(parse_hex_color);
        let attrs = pen.get("attrs").and_then(Value::as_u64).unwrap_or(0) as u32;
        out.push_str(&sgr(fg, bg, attrs));
    } else {
        out.push_str("\x1b[0m");
    }
    let cur = snap.get("cursor");
    let mut crow = usize::try_from(n(cur.and_then(|c| c.get("row")), 0)).unwrap_or(0);
    let ccol = n(cur.and_then(|c| c.get("col")), 0) + 1;
    crow = crow.min(rows.saturating_sub(1)) + 1;
    out.push_str(&format!("\x1b[{crow};{ccol}H"));
    if cur.is_some_and(|c| c.get("visible").and_then(Value::as_bool) != Some(false)) {
        out.push_str("\x1b[?25h");
    } else {
        out.push_str("\x1b[?25l");
    }

    let mut bytes = out.into_bytes();
    if let Some(parser) = snap.get("parser") {
        append_hex(&mut bytes, parser, "utf8Pending");
        let st = parser.get("state").and_then(Value::as_str);
        match st {
            Some("escape") => bytes.push(0x1b),
            Some("escape-intermediate") => {
                bytes.push(0x1b);
                append_hex(&mut bytes, parser, "intermediates");
            }
            Some("csi") => {
                bytes.extend_from_slice(b"\x1b[");
                append_hex(&mut bytes, parser, "params");
                append_hex(&mut bytes, parser, "private");
                append_hex(&mut bytes, parser, "intermediates");
            }
            Some("osc") => {
                bytes.extend_from_slice(b"\x1b]");
                append_hex(&mut bytes, parser, "osc");
            }
            Some("osc-escape") => {
                bytes.extend_from_slice(b"\x1b]\x1b");
                append_hex(&mut bytes, parser, "osc");
            }
            _ => {}
        }
    }
    bytes
}

fn paint_grid(out: &mut String, lines: Option<&Value>, rows: usize) {
    for (r, spans) in lines
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if r >= rows {
            break;
        }
        out.push_str(&format!("\x1b[{};1H", r + 1));
        for span in spans.as_array().into_iter().flatten() {
            let mut it = span.as_array().into_iter().flatten();
            let text = it.next().and_then(Value::as_str).unwrap_or("");
            let fg = it.next().and_then(parse_hex_color);
            let bg = it.next().and_then(parse_hex_color);
            let attrs = it.next().and_then(Value::as_u64).unwrap_or(0) as u32;
            out.push_str(&sgr(fg, bg, attrs));
            let sanitized: String = text
                .chars()
                .map(|c| if (c as u32) < 0x20 { ' ' } else { c })
                .collect();
            out.push_str(&sanitized);
        }
    }
}

fn append_hex(bytes: &mut Vec<u8>, parser: &Value, key: &str) {
    let h = parser.get(key).and_then(Value::as_str);
    if let Some(h) = h
        && let Some(b) = unhex(h)
    {
        bytes.extend_from_slice(&b);
    }
}

fn sgr(fg: Option<(u8, u8, u8)>, bg: Option<(u8, u8, u8)>, attrs: u32) -> String {
    let mut codes: Vec<String> = vec![String::from("0")];
    for (bit, code) in [
        (1, "1"),
        (2, "2"),
        (4, "3"),
        (8, "4"),
        (16, "5"),
        (32, "7"),
        (64, "8"),
        (128, "9"),
    ] {
        if attrs & bit != 0 {
            codes.push(code.to_string());
        }
    }
    if let Some((r, g, b)) = fg {
        codes.push(format!("38;2;{r};{g};{b}"));
    }
    if let Some((r, g, b)) = bg {
        codes.push(format!("48;2;{r};{g};{b}"));
    }
    format!("\x1b[{}m", codes.join(";"))
}

/// `#RRGGBB` (the snapshot spelling) -> RGB channels.
fn parse_hex_color(v: &Value) -> Option<(u8, u8, u8)> {
    let s = v.as_str()?;
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some((
        u8::try_from((v >> 16) & 0xff).unwrap_or(0),
        u8::try_from((v >> 8) & 0xff).unwrap_or(0),
        u8::try_from(v & 0xff).unwrap_or(0),
    ))
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

    #[test]
    fn snapshot_ansi_repaints_the_canvas() {
        let snap = serde_json::json!({
            "schema": "host-snapshot/v1",
            "revision": 7,
            "cols": 80,
            "rows": 2,
            "cursor": { "row": 1, "col": 2, "visible": true },
            "lines": [
                [
                    ["red", "#D7005F", null, 0],
                    [" tail", null, null, 0]
                ],
                [["bold bell\u{7}", null, null, 1]],
            ],
        });
        let s = String::from_utf8(snapshot_ansi(&snap)).unwrap();
        assert!(s.starts_with("\x1b[H\x1b[2J"));
        assert!(s.contains("\x1b[1;1H"));
        // Every span opens with a single SGR carrying reset + styles.
        assert!(s.contains("\x1b[0;38;2;215;0;95mred"));
        assert!(s.contains("\x1b[0m tail"));
        // Attribute bit 1 is bold; the BEL in the cell becomes a space.
        assert!(s.contains("\x1b[0;1mbold bell "));
        assert!(s.ends_with("\x1b[2;3H\x1b[?25h"));

        let hidden = serde_json::json!({
            "rows": 1,
            "cursor": { "row": 0, "col": 0, "visible": false },
            "lines": [[[" ", null, null, 0]]],
        });
        let s = String::from_utf8(snapshot_ansi(&hidden)).unwrap();
        assert!(s.ends_with("\x1b[1;1H\x1b[?25l"));
    }
    #[test]
    fn snapshot_ansi_restores_modes_and_pen() {
        let snap = serde_json::json!({
            "rows": 1,
            "modes": { "autoWrap": true, "bracketedPaste": true, "appCursorKeys": true, "mouse": "click", "mouseSgr": true, "mouseUrxvt": false },
            "pen": { "fg": "#D7005F", "bg": null, "attrs": 0 },
            "cursor": { "row": 0, "col": 1, "visible": true },
            "lines": [[["X", null, null, 0]]],
        });
        let s = String::from_utf8(snapshot_ansi(&snap)).unwrap();
        assert!(s.contains("\x1b[?2004h"));
        assert!(s.contains("\x1b[?1h"));
        assert!(s.contains("\x1b[?1000h"));
        assert!(s.contains("\x1b[?1006h"));
        assert!(s.contains("\x1b[0;38;2;215;0;95m"));
    }

    #[test]
    fn snapshot_ansi_keeps_utf8_pending() {
        let snap = serde_json::json!({
            "rows": 1,
            "cursor": { "row": 0, "col": 0, "visible": false },
            "lines": [[[" ", null, null, 0]]],
            "parser": { "state": "ground", "utf8Pending": "c3" },
        });
        let bytes = snapshot_ansi(&snap);
        assert_eq!(bytes[bytes.len() - 1], 0xc3);
    }

    #[test]
    fn snapshot_ansi_restores_main_and_alt() {
        let snap = serde_json::json!({
            "rows": 2,
            "inAlt": true,
            "mainRows": 1,
            "mainLines": [[["main", null, null, 0]]],
            "cursor": { "row": 1, "col": 0, "visible": true },
            "lines": [
                [["alt", null, null, 0]],
                [[" ", null, null, 0]],
            ],
        });
        let s = String::from_utf8(snapshot_ansi(&snap)).unwrap();
        assert!(s.contains("main"));
        assert!(s.contains("\x1b[?1049h"));
        assert!(s.contains("alt"));
    }
}
