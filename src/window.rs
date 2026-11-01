//! Win32 window layer: window class, message loop, WndProc (spec WIN-001,
//! WIN-002, WIN-005).
//!
//! This module is an FFI boundary (NFR-401): it owns the Win32 message
//! loop and drives the app model, the renderer, and the shells. It never
//! panics on Win32 or Direct2D failures (NFR-216): paint/resize errors are
//! logged, the loop keeps running.
//!
//! Key handling (WIN-005): special keys arrive through `WM_KEYDOWN`/
//! `WM_SYSKEYDOWN` and are encoded by [`crate::keyboard`]; text,
//! Enter/Escape/Backspace and Ctrl-control characters arrive through
//! `WM_CHAR`/`WM_SYSCHAR`. App bindings are Ctrl+Shift+T (new tab),
//! Ctrl+Shift+W (close tab), Ctrl+Shift+C (copy screen), Ctrl+Shift+V
//! (bracketed paste).

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetDpiForWindow,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW,
    HICON, IDC_ARROW, LoadCursorW, MINMAXINFO, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    SW_SHOW, SetTimer, SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage,
    WINDOW_EX_STYLE, WM_CHAR, WM_CLOSE, WM_COPY, WM_CREATE, WM_DESTROY, WM_ERASEBKGND,
    WM_GETMINMAXINFO, WM_KEYDOWN, WM_MOUSEWHEEL, WM_PAINT, WM_PASTE, WM_SETFOCUS, WM_SIZE,
    WM_SYSCHAR, WM_SYSKEYDOWN, WM_TIMER, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::PCWSTR;

use crate::app::App;
use crate::clipboard;
use crate::config::Config;
use crate::keyboard::{self, Modifiers};
use crate::parser::bracketed_paste_bytes;
use crate::render::{CellMetrics, Renderer, measure_cells};
use crate::shell::SpawnOptions;
use crate::theme::Theme;

/// Window class name.
const CLASS_NAME: &str = "rusttyterm";

/// App display name used in the window caption.
const APP_NAME: &str = "rusttyterm";

/// Shell-output poll period in milliseconds.
const POLL_MS: u32 = 33;

/// Timer id for the shell-output poll.
const POLL_TIMER: usize = 1;

/// Lines scrolled per mouse-wheel notch.
const WHEEL_LINES: usize = 3;

/// Smallest cell count the window can be resized to.
const MIN_COLS: usize = 10;
/// Smallest cell count the window can be resized to.
const MIN_ROWS: usize = 4;

/// NUL-terminated UTF-16 for `PCWSTR` parameters.
fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Ctrl/Alt/Shift state read from `GetKeyState`.
fn read_mods() -> Modifiers {
    unsafe {
        Modifiers {
            ctrl: GetKeyState(VK_CONTROL.0 as i32) < 0,
            alt: GetKeyState(VK_MENU.0 as i32) < 0,
            shift: GetKeyState(VK_SHIFT.0 as i32) < 0,
        }
    }
}

/// Per-window state, boxed onto the heap and pointed at by
/// `GWLP_USERDATA` (created in `WM_CREATE`, reclaimed in `WM_DESTROY`).
struct WindowState {
    app: App,
    theme: Theme,
    spawn: SpawnOptions,
    renderer: Option<Renderer>,
    metrics: CellMetrics,
    timer_id: usize,
}

/// Recover the state box from `GWLP_USERDATA`, for the whole lifetime of
/// the window (never reclaimed — `WM_DESTROY` does that).
fn state_ref(hwnd: HWND) -> Option<&'static WindowState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    if ptr == 0 {
        None
    } else {
        // SAFETY: WM_CREATE stored a live Box::into_raw pointer here and
        // WM_DESTROY reclaims it before this can be observed again.
        Some(unsafe { &*(ptr as *const WindowState) })
    }
}

fn state_mut(hwnd: HWND) -> Option<&'static mut WindowState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    if ptr == 0 {
        None
    } else {
        // SAFETY: same invariant as `state_ref`.
        Some(unsafe { &mut *(ptr as *mut WindowState) })
    }
}

/// Timer callback: marshal the tick onto the window thread as WM_TIMER.
unsafe extern "system" fn poll_timer(hwnd: HWND, _id: u32, _arg: usize, _ms: u32) {
    let _ = unsafe { PostMessageW(Some(hwnd), WM_TIMER, WPARAM(0), LPARAM(0)) };
}

/// The window procedure. All Win32 handling for the terminal lives here.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let create = lparam.0 as *const CREATESTRUCTW;
            if create.is_null() {
                return LRESULT(-1);
            }
            let boxed = unsafe { Box::from_raw((*create).lpCreateParams as *mut WindowState) };
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(boxed) as isize) };
            init_window(hwnd);
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let mm = lparam.0 as *mut MINMAXINFO;
            if let (true, Some(s)) = (!mm.is_null(), state_ref(hwnd)) {
                let (w, h) = px_size(s.metrics, MIN_COLS, MIN_ROWS);
                unsafe { (*mm).ptMinTrackSize = POINT { x: w, y: h } };
            }
            LRESULT(0)
        }
        WM_SIZE => {
            let width = ((lparam.0 & 0xFFFF) as u16) as u32;
            let height = (((lparam.0 >> 16) & 0xFFFF) as u16) as u32;
            resize_window(hwnd, width, height);
            LRESULT(0)
        }
        WM_PAINT => {
            // BeginPaint/EndPaint validate the update region. Skipping them
            // keeps WM_PAINT pending forever, and that flood starves
            // WM_TIMER (timer messages are only generated when the queue
            // is otherwise empty) — the shell drain would never run.
            let mut ps = PAINTSTRUCT::default();
            let _dc = unsafe { BeginPaint(hwnd, &mut ps) };
            if let Some(s) = state_mut(hwnd)
                && let Some(r) = &mut s.renderer
                && let Some(tab) = s.app.active_tab()
            {
                // NFR-216: paint() logs and swallows Direct2D failures.
                r.paint(tab.pane(), &s.theme);
            }
            let _ = unsafe { EndPaint(hwnd, &ps) };
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1), // Direct2D owns the whole client area
        WM_KEYDOWN => {
            handle_keydown(hwnd, (wparam.0 & 0xFFFF) as u16);
            LRESULT(0)
        }
        WM_SYSKEYDOWN => {
            let vk = (wparam.0 & 0xFFFF) as u16;
            // Alt+Tab and friends must keep their system behaviour.
            if vk == 0x09 {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            handle_keydown(hwnd, vk);
            LRESULT(0)
        }
        WM_CHAR => {
            handle_char(hwnd, wparam.0 as u32, false);
            LRESULT(0)
        }
        WM_SYSCHAR => {
            handle_char(hwnd, wparam.0 as u32, true);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            handle_wheel(hwnd, ((wparam.0 >> 16) & 0xFFFF) as u16);
            LRESULT(0)
        }
        WM_TIMER => {
            drain_and_cleanup(hwnd);
            LRESULT(0)
        }
        WM_SETFOCUS => {
            let _ = unsafe { SetFocus(Some(hwnd)) };
            LRESULT(0)
        }
        WM_COPY => {
            copy_screen(hwnd);
            LRESULT(0)
        }
        WM_PASTE => {
            paste_clipboard(hwnd);
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(ptr) = reclaim_state(hwnd) {
                // SAFETY: WM_CREATE stored this pointer; the window is
                // going away, so take ownership back and drop the app
                // (which closes every shell).
                let mut state = unsafe { Box::from_raw(ptr) };
                state.app.close_all_shells();
            }
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Take the state pointer off `GWLP_USERDATA` (clears the slot).
fn reclaim_state(hwnd: HWND) -> Option<*mut WindowState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    if ptr == 0 {
        return None;
    }
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
    Some(ptr as *mut WindowState)
}

/// WM_CREATE follow-up: show the window, build the renderer, size the
/// grid, spawn the first shell, start the output-poll timer.
fn init_window(hwnd: HWND) {
    let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
    let dpi = unsafe {
        let d = GetDpiForWindow(hwnd);
        if d == 0 { GetDpiForSystem() } else { d }
    };
    let (init_cols, init_rows) = state_ref(hwnd).map_or((80, 24), |s| (s.spawn.cols, s.spawn.rows));
    let metrics = match measure_cells(dpi) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("rusttyterm: font measurement failed: {e}");
            return;
        }
    };
    if let Some(s) = state_mut(hwnd) {
        s.metrics = metrics;
    }
    let renderer = match Renderer::new(hwnd, dpi, init_cols, init_rows) {
        Ok(r) => Some(r),
        Err(e) => {
            // NFR-216: the app degrades to a live-but-blind terminal.
            eprintln!("rusttyterm: Direct2D renderer unavailable: {e}");
            None
        }
    };
    if let Some(s) = state_mut(hwnd) {
        s.renderer = renderer;
        let (cols, rows) = s
            .renderer
            .as_ref()
            .map_or((init_cols, init_rows), |r| r.grid_size());
        let cols = cols.max(MIN_COLS);
        let rows = rows.max(MIN_ROWS);
        // Spawn first, then resize: a resize must reach a live ConPTY, and
        // the shell must start at the window's real grid size (NFR-216:
        // these failures are logged, never fatal).
        if let Err(e) = s.app.spawn_active(&s.spawn.clone()) {
            eprintln!("rusttyterm: shell spawn failed: {e}");
        }
        if let Err(e) = s.app.resize_active(cols, rows) {
            eprintln!("rusttyterm: initial resize failed: {e}");
        }
        s.timer_id = unsafe { SetTimer(Some(hwnd), POLL_TIMER, POLL_MS, Some(poll_timer)) };
        set_title(hwnd, &s.app);
        let _ = unsafe { SetFocus(Some(hwnd)) };
    }
}

/// WM_SIZE follow-up: resize render target -> grid -> ConPTY (NFR resize
/// chain, spec NFR).
fn resize_window(hwnd: HWND, width_px: u32, height_px: u32) {
    let Some(s) = state_mut(hwnd) else { return };
    let Some(r) = &mut s.renderer else { return };
    r.resize(width_px, height_px);
    let (cols, rows) = r.grid_size().max((MIN_COLS, MIN_ROWS));
    if s.app.resize_active(cols, rows).is_err() {
        eprintln!("rusttyterm: ConPTY resize failed");
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// WM_KEYDOWN/WM_SYSKEYDOWN: app bindings first, then VT encoding.
fn handle_keydown(hwnd: HWND, vk: u16) {
    let mods = read_mods();
    if mods.ctrl && mods.shift {
        match vk {
            0x54 => {
                // Ctrl+Shift+T: new tab
                new_tab(hwnd);
                return;
            }
            0x57 => {
                // Ctrl+Shift+W: close tab
                close_tab(hwnd);
                return;
            }
            0x43 => {
                // Ctrl+Shift+C: copy the visible screen
                copy_screen(hwnd);
                return;
            }
            0x56 => {
                // Ctrl+Shift+V: bracketed paste
                paste_clipboard(hwnd);
                return;
            }
            _ => {}
        }
    }
    let Some(s) = state_ref(hwnd) else { return };
    let app_cursor = s
        .app
        .active_tab()
        .is_some_and(|t| t.pane().parser().application_cursor_keys);
    let bytes = keyboard::encode_key(vk, mods, app_cursor);
    if !bytes.is_empty()
        && let Some(s) = state_mut(hwnd)
    {
        let _ = s.app.send_input_active(&bytes);
    }
}

/// WM_CHAR/WM_SYSCHAR: text, Enter, Escape, Backspace, Ctrl-control.
fn handle_char(hwnd: HWND, code: u32, sys: bool) {
    let Some(ch) = char::from_u32(code) else {
        return;
    };
    // VK_PACKET surrogates arrive as WM_CHAR; ignore unpaired halves.
    if (0xD800..0xDC00).contains(&code) {
        return;
    }
    let mods = if sys {
        let mut m = read_mods();
        m.alt = true;
        m
    } else {
        read_mods()
    };
    let Some(s) = state_mut(hwnd) else { return };
    let bytes = keyboard::encode_char(ch, mods);
    let _ = s.app.send_input_active(&bytes);
}

/// WM_MOUSEWHEEL: scroll the viewport through the scrollback.
fn handle_wheel(hwnd: HWND, delta_u16: u16) {
    let delta = delta_u16 as i16;
    let lines = if read_mods().ctrl {
        WHEEL_LINES * 2
    } else {
        WHEEL_LINES
    };
    let Some(s) = state_mut(hwnd) else { return };
    if let Some(tab) = s.app.active_tab_mut() {
        let grid = tab.pane_mut().grid_mut();
        if delta < 0 {
            grid.scroll_up(lines);
        } else {
            grid.scroll_down(lines);
        }
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// WM_TIMER: drain every shell, close exited tabs, refresh the caption.
fn drain_and_cleanup(hwnd: HWND) {
    let Some(s) = state_mut(hwnd) else { return };
    s.app.drain_shells();
    // Repaint only when a grid actually changed (NFR-113: no idle repaints).
    if s.app.take_dirty() {
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    }
    let dead: Vec<usize> = s
        .app
        .tabs()
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            matches!(
                t.pane().shell().state(),
                crate::shell::ShellState::Inactive | crate::shell::ShellState::Exited(_)
            )
        })
        .map(|(i, _)| i)
        .collect();
    for idx in dead.into_iter().rev() {
        if s.app.close_tab(idx) {
            let _ = unsafe { DestroyWindow(hwnd) };
            return;
        }
    }
    set_title(hwnd, &s.app);
}

/// Ctrl+Shift+T: append a tab and spawn a shell at the current grid size.
fn new_tab(hwnd: HWND) {
    let Some(s) = state_mut(hwnd) else { return };
    s.app.add_tab();
    let (cols, rows) = s
        .renderer
        .as_ref()
        .map_or((s.spawn.cols, s.spawn.rows), |r| r.grid_size());
    let opts = SpawnOptions {
        program: s.spawn.program.clone(),
        args: s.spawn.args.clone(),
        cols: cols.max(MIN_COLS),
        rows: rows.max(MIN_ROWS),
    };
    if let Err(e) = s.app.spawn_active(&opts) {
        eprintln!("rusttyterm: shell spawn failed: {e}");
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Ctrl+Shift+W: close the active tab; closing the last tab closes the
/// window (WM_DESTROY reclaims the state and quits the loop).
fn close_tab(hwnd: HWND) {
    let Some(s) = state_mut(hwnd) else { return };
    let idx = s.app.active_index();
    if s.app.close_tab(idx) {
        let _ = unsafe { DestroyWindow(hwnd) };
        return;
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Ctrl+Shift+C: copy the visible viewport text (WIN-004 MVP).
fn copy_screen(hwnd: HWND) {
    let Some(s) = state_ref(hwnd) else { return };
    let Some(tab) = s.app.active_tab() else {
        return;
    };
    let text = Renderer::visible_text(tab.pane().grid());
    if !text.is_empty() && !clipboard::set_text(&text) {
        eprintln!("rusttyterm: clipboard copy failed");
    }
}

/// Ctrl+Shift+V: paste the clipboard, bracketed when the shell asked
/// for it (WIN-004 MVP).
fn paste_clipboard(hwnd: HWND) {
    let Some(s) = state_ref(hwnd) else { return };
    let Some(text) = clipboard::get_text() else {
        return;
    };
    let enabled = s
        .app
        .active_tab()
        .is_some_and(|t| t.pane().parser().bracketed_paste);
    let bytes = bracketed_paste_bytes(&text, enabled);
    if let Some(s) = state_mut(hwnd) {
        let _ = s.app.send_input_active(&bytes);
    }
}

/// Window caption from the active tab title.
fn set_title(hwnd: HWND, app: &App) {
    let title = app
        .active_tab()
        .map(|t| format!("{} — {APP_NAME}", t.title()))
        .unwrap_or_else(|| APP_NAME.to_string());
    let wide = wstr(&title);
    let _ = unsafe { SetWindowTextW(hwnd, PCWSTR::from_raw(wide.as_ptr())) };
}

/// Client-area pixel size for a `cols` x `rows` grid.
fn px_size(metrics: CellMetrics, cols: usize, rows: usize) -> (i32, i32) {
    let w = (cols.max(1) as f32 * metrics.cell_width).ceil() as i32;
    let h = (rows.max(1) as f32 * metrics.cell_height).ceil() as i32;
    (w.max(16), h.max(16))
}

/// Open the terminal window and run the message loop until the last tab
/// closes (spec WIN-001: this is the UI thread that owns ConPTY).
///
/// # Errors
///
/// Fails when the window class cannot be registered or the window cannot
/// be created.
pub fn run(cfg: &Config, theme: Theme, spawn: SpawnOptions) -> anyhow::Result<()> {
    // Per-monitor DPI v2 before any window metrics exist (WIN-002).
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let dpi = unsafe {
        let d = GetDpiForSystem();
        if d == 0 { 96 } else { d }
    };
    let metrics = measure_cells(dpi)?;
    let cols = cfg.window.cols.max(MIN_COLS);
    let rows = cfg.window.rows.max(MIN_ROWS);
    let (cell_w, cell_h) = px_size(metrics, cols, rows);
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: cell_w,
        bottom: cell_h,
    };
    let style = WS_OVERLAPPEDWINDOW | WS_VISIBLE;
    unsafe { AdjustWindowRect(&mut rect, style, false) }?;

    let state = Box::new(WindowState {
        app: App::new(),
        theme,
        spawn,
        renderer: None,
        metrics,
        timer_id: 0,
    });
    let state_param = Box::into_raw(state) as *const c_void;

    let hinstance = unsafe { GetModuleHandleW(None) }?;
    let class_name = wstr(CLASS_NAME);
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }?;
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance.into(),
        hIcon: HICON::default(),
        hCursor: cursor,
        hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(anyhow::anyhow!(
            "rusttyterm: window class registration failed"
        ));
    }

    let title = wstr(APP_NAME);
    let created = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            None,
            None,
            Some(hinstance.into()),
            Some(state_param),
        )
    };
    if let Err(e) = created {
        // The window never existed to reclaim the box; free it here.
        unsafe { drop(Box::from_raw(state_param as *mut WindowState)) };
        return Err(anyhow::anyhow!("rusttyterm: window creation failed: {e}"));
    }

    // Message loop: quit when GetMessageW reports WM_QUIT (PostQuitMessage
    // in WM_DESTROY) or the queue fails (0 = error, not "no message").
    let mut msg = MSG::default();
    loop {
        let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if ret.0 <= 0 {
            break;
        }
        let _ = unsafe { TranslateMessage(&msg) };
        let _ = unsafe { DispatchMessageW(&msg) };
    }
    Ok(())
}
