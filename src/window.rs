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
//! `WM_CHAR`/`WM_SYSCHAR`. App bindings: Ctrl+Shift+T (new tab),
//! Ctrl+Shift+W (close pane, else tab), Ctrl+Shift+T/W/C/V, Ctrl+Shift+]/[
//! (next/prev tab), Ctrl+Shift+Q (quit), Ctrl+Shift+Enter (split right),
//! Ctrl+Shift+- (split below), Ctrl+Alt+arrows (pane focus),
//! Ctrl+Alt+Shift+arrows (pane resize), F11 (fullscreen),
//! Ctrl+Shift+C (copy selection, else screen), Ctrl+Shift+V (paste).
//! Mouse (WIN-004): click focuses a pane, drag-selects text, double-click
//! selects a word, middle-click pastes, wheel scrolls the pane under the
//! cursor, tab-bar clicks switch tabs / press `+`.
//!
//! Menus (MENU-001..015): native Win32 File/Edit/View/Window/Help menus
//! expose tab, pane, split, search, theme, profile, clipboard, window and
//! help commands with visible shortcuts.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromWindow, PAINTSTRUCT, ScreenToClient,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetDpiForWindow,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_CONTROL, VK_MENU,
    VK_SHIFT,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, AppendMenuW, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
    CheckMenuItem, CreateMenu, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DestroyWindow, DispatchMessageW, EnableMenuItem, GWL_EXSTYLE, GWL_STYLE, GWLP_USERDATA,
    GetClientRect, GetCursorPos, GetMenu, GetMessageW, GetSubMenu, GetWindowLongPtrW,
    GetWindowRect, HICON, HMENU, IDC_ARROW, IDC_IBEAM, IDYES, IsZoomed, LoadCursorW, MB_DEFBUTTON2,
    MB_ICONWARNING, MB_OK, MB_YESNO, MF_BYCOMMAND, MF_CHECKED, MF_ENABLED, MF_GRAYED, MF_POPUP,
    MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MINMAXINFO, MSG, MessageBoxW, PostMessageW,
    PostQuitMessage, RegisterClassW, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOW,
    SWP_FRAMECHANGED, SWP_NOZORDER, SetCursor, SetMenu, SetTimer, SetWindowLongPtrW, SetWindowPos,
    SetWindowTextW, ShowWindow, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE,
    WM_CHAR, WM_CLOSE, WM_COMMAND, WM_CONTEXTMENU, WM_COPY, WM_CREATE, WM_DESTROY, WM_ERASEBKGND,
    WM_GETMINMAXINFO, WM_INITMENUPOPUP, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_PASTE, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SETCURSOR, WM_SETFOCUS, WM_SIZE, WM_SYSCHAR, WM_SYSKEYDOWN, WM_TIMER, WNDCLASSW,
    WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
};
use windows::core::{PCWSTR, Result};

use crate::app::{App, PaneClose};
use crate::clipboard;
use crate::config::{self, Config, FontConfig, TabBar};
use crate::keyboard::{self, Modifiers};
use crate::pane::Pane;
use crate::parser::{MouseProtocol, bracketed_paste_bytes, mouse_report_bytes};
use crate::render::{CellMetrics, Renderer, measure_cells, px_to_dips};
use crate::search::SearchState;
use crate::selection::{Pos, Selection};
use crate::shell::SpawnOptions;
use crate::split::{Dir, Rect, Side};
use crate::tabbar::{self, Hit, TAB_BAR_HEIGHT};
use crate::theme::{self, Theme};

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

/// Native menu command ids (MENU-001..015). They live above the reserved
/// system command range and below the generated profile/theme bases.
const MENU_NEW_TAB: usize = 0xE801;
const MENU_CLOSE_TAB: usize = 0xE802;
const MENU_SPLIT_RIGHT: usize = 0xE803;
const MENU_SPLIT_DOWN: usize = 0xE804;
const MENU_PREFERENCES: usize = 0xE805;
const MENU_QUIT: usize = 0xE806;
const MENU_COPY: usize = 0xE811;
const MENU_PASTE: usize = 0xE812;
const MENU_SEARCH: usize = 0xE821;
const MENU_FULLSCREEN: usize = 0xE822;
const MENU_NEXT_TAB: usize = 0xE831;
const MENU_PREV_TAB: usize = 0xE832;
const MENU_MINIMIZE: usize = 0xE833;
const MENU_MAXIMIZE: usize = 0xE834;
const MENU_CLOSE_WINDOW: usize = 0xE835;
const MENU_ABOUT: usize = 0xE841;
const MENU_THEME_BASE: usize = 0xF000;
const MENU_PROFILE_BASE: usize = 0xF100;

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

/// Built-in shell selector entries shown under File > Profiles.
///
/// This is deliberately small for v0.2: profile config (`CONFIG-002`,
/// `SHELL-012`) needs TOML/profile wiring, while the menu requirement only
/// needs a native profile selector. Selecting an entry starts a new tab
/// with that shell; splits inherit the selector's current shell.
#[derive(Clone, Copy)]
struct BuiltinProfile {
    name: &'static str,
    program: &'static str,
    args: &'static [&'static str],
}

const PROFILES: [BuiltinProfile; 6] = [
    BuiltinProfile {
        name: "PowerShell",
        program: "powershell.exe",
        args: &[],
    },
    BuiltinProfile {
        name: "Command Prompt",
        program: "cmd.exe",
        args: &[],
    },
    BuiltinProfile {
        name: "Git Bash",
        program: r"C:\Program Files\Git\bin\bash.exe",
        args: &["-l"],
    },
    BuiltinProfile {
        name: "WSL",
        program: "wsl.exe",
        args: &[],
    },
    BuiltinProfile {
        name: "MSYS2",
        program: r"C:\msys64\usr\bin\bash.exe",
        args: &["-l"],
    },
    BuiltinProfile {
        name: "Cygwin",
        program: r"C:\cygwin64\bin\bash.exe",
        args: &["-l"],
    },
];

fn append_menu_item(parent: HMENU, text: &str, id: usize) -> Result<()> {
    let wide = wstr(text);
    unsafe { AppendMenuW(parent, MF_STRING, id, PCWSTR(wide.as_ptr())) }
}

fn append_menu_separator(parent: HMENU) -> Result<()> {
    unsafe { AppendMenuW(parent, MF_SEPARATOR, 0, PCWSTR::null()) }
}

fn append_menu_submenu(parent: HMENU, text: &str, submenu: HMENU) -> Result<()> {
    let wide = wstr(text);
    unsafe { AppendMenuW(parent, MF_POPUP, submenu.0 as usize, PCWSTR(wide.as_ptr())) }
}

/// Build the native menu bar (MENU-001..015). Menu labels carry the visible
/// keyboard shortcut text; the app's existing keybindings execute them.
fn build_menu() -> Result<HMENU> {
    unsafe {
        let menu = CreateMenu()?;

        let file = CreatePopupMenu()?;
        append_menu_item(file, "&New Tab\tCtrl+Shift+T", MENU_NEW_TAB)?;
        append_menu_item(file, "Close Pane/&Tab\tCtrl+Shift+W", MENU_CLOSE_TAB)?;
        append_menu_separator(file)?;
        append_menu_item(file, "&Split Right\tCtrl+Shift+Enter", MENU_SPLIT_RIGHT)?;
        append_menu_item(file, "Split &Below\tCtrl+Shift+-", MENU_SPLIT_DOWN)?;
        append_menu_separator(file)?;
        let profiles = CreatePopupMenu()?;
        for (idx, profile) in PROFILES.iter().enumerate() {
            append_menu_item(
                profiles,
                &format!("&{}", profile.name),
                MENU_PROFILE_BASE + idx,
            )?;
        }
        append_menu_submenu(file, "&Profiles", profiles)?;
        append_menu_separator(file)?;
        append_menu_item(file, "&Preferences", MENU_PREFERENCES)?;
        append_menu_separator(file)?;
        append_menu_item(file, "E&xit\tCtrl+Shift+Q", MENU_QUIT)?;
        append_menu_submenu(menu, "&File", file)?;

        let edit = CreatePopupMenu()?;
        append_menu_item(edit, "&Copy\tCtrl+Shift+C", MENU_COPY)?;
        append_menu_item(edit, "&Paste\tCtrl+Shift+V", MENU_PASTE)?;
        append_menu_submenu(menu, "&Edit", edit)?;

        let view = CreatePopupMenu()?;
        append_menu_item(view, "&Search\tCtrl+F", MENU_SEARCH)?;
        append_menu_separator(view)?;
        append_menu_item(view, "Split &Right\tCtrl+Shift+Enter", MENU_SPLIT_RIGHT)?;
        append_menu_item(view, "Split Be&low\tCtrl+Shift+-", MENU_SPLIT_DOWN)?;
        append_menu_separator(view)?;
        append_menu_item(view, "Full &Screen\tF11", MENU_FULLSCREEN)?;
        append_menu_separator(view)?;
        let themes = CreatePopupMenu()?;
        for (idx, name) in theme::PRESETS.iter().enumerate() {
            append_menu_item(themes, &format!("&{name}"), MENU_THEME_BASE + idx)?;
        }
        append_menu_submenu(view, "&Themes", themes)?;
        append_menu_submenu(menu, "&View", view)?;

        let window = CreatePopupMenu()?;
        append_menu_item(window, "&Next Tab\tCtrl+Shift+]", MENU_NEXT_TAB)?;
        append_menu_item(window, "&Previous Tab\tCtrl+Shift+[", MENU_PREV_TAB)?;
        append_menu_separator(window)?;
        append_menu_item(window, "&Minimize", MENU_MINIMIZE)?;
        append_menu_item(window, "Maxi&mize / Restore", MENU_MAXIMIZE)?;
        append_menu_separator(window)?;
        append_menu_item(window, "&Close Window", MENU_CLOSE_WINDOW)?;
        append_menu_submenu(menu, "&Window", window)?;

        let help = CreatePopupMenu()?;
        append_menu_item(help, "&About rusttyterm", MENU_ABOUT)?;
        append_menu_submenu(menu, "&Help", help)?;

        Ok(menu)
    }
}

fn check_menu_item(menu: HMENU, id: usize, checked: bool) {
    let flags = MF_BYCOMMAND | if checked { MF_CHECKED } else { MF_UNCHECKED };
    unsafe {
        let _ = CheckMenuItem(menu, id as u32, flags.0);
    }
}

fn enable_menu_item(menu: HMENU, id: usize, enabled: bool) {
    let flags = MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED };
    unsafe {
        let _ = EnableMenuItem(menu, id as u32, flags);
    }
}

fn submenu_of(menu: HMENU, idx: i32) -> Option<HMENU> {
    let submenu = unsafe { GetSubMenu(menu, idx) };
    (!submenu.0.is_null()).then_some(submenu)
}

fn same_menu(a: HMENU, b: HMENU) -> bool {
    a.0 == b.0
}

fn active_profile(s: &WindowState) -> &BuiltinProfile {
    &PROFILES[s.profile.min(PROFILES.len() - 1)]
}

fn profile_opts(s: &WindowState, cols: usize, rows: usize) -> SpawnOptions {
    let profile = active_profile(s);
    SpawnOptions {
        program: profile.program.to_string(),
        args: profile.args.iter().map(|arg| (*arg).to_string()).collect(),
        cols,
        rows,
    }
}

fn active_theme_index(s: &WindowState) -> usize {
    theme::PRESETS
        .iter()
        .position(|name| *name == s.theme.name)
        .unwrap_or(0)
}

fn theme_command(id: usize) -> Option<usize> {
    id.checked_sub(MENU_THEME_BASE)
        .filter(|idx| *idx < theme::PRESETS.len())
}

fn profile_command(id: usize) -> Option<usize> {
    id.checked_sub(MENU_PROFILE_BASE)
        .filter(|idx| *idx < PROFILES.len())
}

/// Update check marks and enabled state for the popup about to open.
fn update_menu_checks(hwnd: HWND, wparam: WPARAM) {
    let popup = HMENU(wparam.0 as *mut c_void);
    let menu = unsafe { GetMenu(hwnd) };
    if menu.0.is_null() {
        return;
    }
    let Some(state) = state_ref(hwnd) else {
        return;
    };
    let tabs = state.app.tab_count();

    if let Some(file) = submenu_of(menu, 0)
        && same_menu(popup, file)
    {
        enable_menu_item(file, MENU_CLOSE_TAB, tabs > 0);
        if let Some(profiles) = submenu_of(file, 6) {
            for idx in 0..PROFILES.len() {
                check_menu_item(profiles, MENU_PROFILE_BASE + idx, state.profile == idx);
            }
        }
    }

    if let Some(view) = submenu_of(menu, 2)
        && same_menu(popup, view)
    {
        if let Some(themes) = submenu_of(view, 7) {
            let active = active_theme_index(state);
            for idx in 0..theme::PRESETS.len() {
                check_menu_item(themes, MENU_THEME_BASE + idx, idx == active);
            }
        }
        enable_menu_item(view, MENU_SEARCH, tabs > 0);
        check_menu_item(view, MENU_FULLSCREEN, state.fullscreen.is_some());
    }

    if let Some(window) = submenu_of(menu, 3)
        && same_menu(popup, window)
    {
        enable_menu_item(window, MENU_NEXT_TAB, tabs > 1);
        enable_menu_item(window, MENU_PREV_TAB, tabs > 1);
        enable_menu_item(window, MENU_CLOSE_WINDOW, tabs > 0);
    }
}

fn handle_command(hwnd: HWND, wparam: WPARAM) {
    let id = wparam.0 & 0xFFFF;
    if let Some(idx) = theme_command(id) {
        set_theme(hwnd, idx);
        return;
    }
    if let Some(idx) = profile_command(id) {
        open_profile_tab(hwnd, idx);
        return;
    }
    match id {
        MENU_NEW_TAB => new_tab(hwnd),
        MENU_CLOSE_TAB => close_pane_or_tab(hwnd),
        MENU_SPLIT_RIGHT => split_pane(hwnd, Dir::Vertical),
        MENU_SPLIT_DOWN => split_pane(hwnd, Dir::Horizontal),
        MENU_SEARCH => toggle_search(hwnd),
        MENU_FULLSCREEN => toggle_fullscreen(hwnd),
        MENU_COPY => copy_screen(hwnd),
        MENU_PASTE => paste_clipboard(hwnd),
        MENU_NEXT_TAB => switch_tab_rel(hwnd, 1),
        MENU_PREV_TAB => switch_tab_rel(hwnd, -1),
        MENU_MINIMIZE => unsafe {
            let _ = ShowWindow(hwnd, SW_MINIMIZE);
        },
        MENU_MAXIMIZE => unsafe {
            if IsZoomed(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            } else {
                let _ = ShowWindow(hwnd, SW_MAXIMIZE);
            }
        },
        MENU_CLOSE_WINDOW | MENU_QUIT => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        MENU_PREFERENCES => open_preferences(hwnd),
        MENU_ABOUT => show_about(hwnd),
        _ => {}
    }
}

fn set_theme(hwnd: HWND, idx: usize) {
    let Some(name) = theme::PRESETS.get(idx) else {
        return;
    };
    if let Some(s) = state_mut(hwnd) {
        s.theme = Theme::preset(name).unwrap_or_default();
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

fn open_profile_tab(hwnd: HWND, idx: usize) {
    let Some(name) = PROFILES.get(idx).map(|p| p.name) else {
        return;
    };
    if let Some(s) = state_mut(hwnd) {
        s.profile = idx;
    }
    new_tab_with_title(hwnd, Some(name));
}

fn open_preferences(hwnd: HWND) {
    let Some(path) = config::config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if !path.exists() {
        let _ = std::fs::write(&path, b"# rusttyterm config\n");
    }
    let op = wstr("open");
    let file: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let _ = ShellExecuteW(
            Some(hwnd),
            PCWSTR(op.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOW,
        );
    }
}

fn show_about(hwnd: HWND) {
    let text = wstr(&format!(
        "rusttyterm {}\n\nA native Windows terminal emulator.\nMIT licensed. No telemetry.",
        env!("CARGO_PKG_VERSION")
    ));
    let caption = wstr("About rusttyterm");
    unsafe {
        let _ = MessageBoxW(
            Some(hwnd),
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_OK,
        );
    }
}

/// Per-window state, boxed onto the heap and pointed at by
/// `GWLP_USERDATA` (created in `WM_CREATE`, reclaimed in `WM_DESTROY`).
struct WindowState {
    app: App,
    theme: Theme,
    spawn: SpawnOptions,
    font: FontConfig,
    tab_bar: TabBar,
    renderer: Option<Renderer>,
    metrics: CellMetrics,
    timer_id: usize,
    /// Anchor cell of the in-progress mouse text selection drag
    /// (absolute grid coordinates in the pane the drag started on) plus
    /// that pane's flatten index; `None` when not dragging (WIN-004,
    /// SPLIT-004: drags stay inside their pane).
    drag_anchor: Option<(Pos, usize)>,
    /// Last grid click as (absolute row, col, GetTickCount64 ms): used to
    /// detect double clicks the system does not coalesce into
    /// `WM_LBUTTONDBLCLK` (injected/synthesized input never coalesces).
    last_click: Option<(usize, usize, i64)>,
    /// Set when an app key binding consumed a `WM_KEYDOWN`; the matching
    /// `WM_CHAR` Windows posts afterwards must not reach the PTY.
    suppress_char: bool,
    /// Saved window rect + styles while fullscreen (F11), `None` in
    /// windowed mode.
    fullscreen: Option<(RECT, isize, isize)>,
    /// Search overlay state (SEARCH-001..015); `None` when the overlay
    /// is closed. While active, keystrokes edit the query instead of
    /// reaching the shell.
    search: Option<SearchState>,
    /// Index of the selected built-in profile (MENU-008). New tabs and
    /// split panes spawn it.
    profile: usize,
}

impl WindowState {
    /// Tab-bar strip height in DIPs for the current tab count (TAB-010):
    /// `always` shows it, `auto` shows it only with more than one tab,
    /// `hidden` never shows it.
    fn bar_height(&self) -> f32 {
        match self.tab_bar {
            TabBar::Always => TAB_BAR_HEIGHT,
            TabBar::Auto if self.app.tab_count() > 1 => TAB_BAR_HEIGHT,
            TabBar::Auto | TabBar::Hidden => 0.0,
        }
    }
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
                let (w, h) = px_size(s.metrics, MIN_COLS, MIN_ROWS, s.bar_height());
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
            {
                // NFR-216: paint() logs and swallows Direct2D failures.
                r.paint(&s.app, &s.theme, s.search.as_ref());
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
            if take_suppressed_char(hwnd) {
                return LRESULT(0);
            }
            handle_char(hwnd, wparam.0 as u32, false);
            LRESULT(0)
        }
        WM_SYSCHAR => {
            if take_suppressed_char(hwnd) {
                return LRESULT(0);
            }
            handle_char(hwnd, wparam.0 as u32, true);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            handle_wheel(hwnd, wparam, lparam);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            handle_lbutton_down(hwnd, lparam);
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            handle_dblclick(hwnd, lparam);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            handle_mouse_move(hwnd, wparam, lparam);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            handle_lbutton_up(hwnd, lparam);
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            handle_rbutton_down(hwnd, lparam);
            LRESULT(0)
        }
        WM_RBUTTONUP => LRESULT(0),
        WM_MBUTTONDOWN => {
            if handle_mbutton_down(hwnd, lparam) {
                LRESULT(0)
            } else {
                // Middle-click paste (xterm convention, WIN-004).
                paste_clipboard(hwnd);
                LRESULT(0)
            }
        }
        WM_CONTEXTMENU => {
            handle_context_menu(hwnd, lparam);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            set_cursor_for_position(wparam);
            LRESULT(1)
        }
        WM_TIMER => {
            drain_and_cleanup(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            handle_command(hwnd, wparam);
            LRESULT(0)
        }
        WM_INITMENUPOPUP => {
            update_menu_checks(hwnd, wparam);
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
    // Native menu bar (MENU-001..005). A menu failure leaves the terminal
    // usable, matching the no-fatal-UI-failure rule.
    if let Ok(menu) = build_menu() {
        unsafe {
            let _ = SetMenu(hwnd, Some(menu));
        }
    }
    let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
    let dpi = unsafe {
        let d = GetDpiForWindow(hwnd);
        if d == 0 { GetDpiForSystem() } else { d }
    };
    let (init_cols, init_rows, font, bar) = state_ref(hwnd)
        .map_or((80, 24, FontConfig::default(), TAB_BAR_HEIGHT), |s| {
            (s.spawn.cols, s.spawn.rows, s.font.clone(), s.bar_height())
        });
    let metrics = match measure_cells(dpi, &font) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("rusttyterm: font measurement failed: {e}");
            return;
        }
    };
    if let Some(s) = state_mut(hwnd) {
        s.metrics = metrics;
    }
    let renderer = match Renderer::new(hwnd, dpi, &font, init_cols, init_rows, bar) {
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

/// Tab-count change follow-up: re-apply the tab-bar visibility rule. The
/// strip height carves grid area off the top, so when it changes the grid
/// and the ConPTY must follow (TAB-010, `tab-bar = "auto"/"hidden"`).
fn sync_tab_bar(hwnd: HWND) {
    let Some(s) = state_mut(hwnd) else { return };
    let bar = s.bar_height();
    let Some(r) = &mut s.renderer else { return };
    if r.tab_bar_height() == bar {
        return;
    }
    r.set_tab_bar_height(bar);
    let (cols, rows) = r.grid_size().max((MIN_COLS, MIN_ROWS));
    if s.app.resize_active(cols, rows).is_err() {
        eprintln!("rusttyterm: ConPTY resize failed");
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// WM_KEYDOWN/WM_SYSKEYDOWN: app bindings first, then VT encoding.
fn handle_keydown(hwnd: HWND, vk: u16) {
    let mods = read_mods();
    // Ctrl+F opens or closes the search overlay (SEARCH-002).
    if mods.ctrl && !mods.shift && !mods.alt && vk == 0x46 {
        toggle_search(hwnd);
        return;
    }
    // While search is active, special keys drive search and every other
    // keydown is left for WM_CHAR, which edits the query instead of the PTY.
    if state_ref(hwnd).is_some_and(|s| s.search.is_some()) {
        if handle_search_keydown(hwnd, vk, mods) {
            return;
        }
        return;
    }
    if vk == 0x7A {
        // F11: fullscreen toggle
        toggle_fullscreen(hwnd);
        return;
    }
    if mods.ctrl && mods.shift {
        let consumed = match vk {
            0x54 => {
                // Ctrl+Shift+T: new tab
                new_tab(hwnd);
                true
            }
            0x57 => {
                // Ctrl+Shift+W: close focused pane, else close tab
                close_pane_or_tab(hwnd);
                true
            }
            0x43 => {
                // Ctrl+Shift+C: copy selection (else visible screen)
                copy_screen(hwnd);
                true
            }
            0x56 => {
                // Ctrl+Shift+V: bracketed paste
                paste_clipboard(hwnd);
                true
            }
            0x51 => {
                // Ctrl+Shift+Q: quit (closes the window, WM_DESTROY quits)
                let _ = unsafe { DestroyWindow(hwnd) };
                true
            }
            0xDD => {
                // Ctrl+Shift+]: next tab
                switch_tab_rel(hwnd, 1);
                true
            }
            0xDB => {
                // Ctrl+Shift+[: previous tab
                switch_tab_rel(hwnd, -1);
                true
            }
            0x0A => {
                // Ctrl+Shift+Enter: split the focused pane, new pane right
                // (SPLIT-005)
                split_pane(hwnd, Dir::Vertical);
                true
            }
            0xBD => {
                // Ctrl+Shift+-: split the focused pane, new pane below
                // (SPLIT-005)
                split_pane(hwnd, Dir::Horizontal);
                true
            }
            _ => false,
        };
        if consumed {
            // Swallow the WM_CHAR Windows posts for this key so the
            // shortcut's control byte never reaches the PTY.
            if let Some(s) = state_mut(hwnd) {
                s.suppress_char = true;
            }
            return;
        }
    }
    // Ctrl+Alt+arrows: pane focus (SPLIT-008). Ctrl+Alt+Shift+arrows:
    // pane resize (SPLIT-009). Unsplit tabs fall through to normal VT
    // encoding so the shells keep receiving their arrow keys.
    if mods.ctrl && mods.alt {
        let side = match vk {
            0x25 => Some(Side::Left),
            0x26 => Some(Side::Up),
            0x27 => Some(Side::Right),
            0x28 => Some(Side::Down),
            _ => None,
        };
        if let Some(side) = side {
            let moved = state_mut(hwnd).is_some_and(|s| {
                if mods.shift {
                    // Shift grows/shrinks the focused pane across its
                    // nearest split: left/up shrink, right/down grow.
                    let (dir, delta) = match side {
                        Side::Left => (Dir::Vertical, -App::ratio_step()),
                        Side::Right => (Dir::Vertical, App::ratio_step()),
                        Side::Up => (Dir::Horizontal, -App::ratio_step()),
                        Side::Down => (Dir::Horizontal, App::ratio_step()),
                    };
                    s.app.adjust_ratio(dir, delta)
                } else {
                    s.app.focus_neighbor(side)
                }
            });
            if moved {
                if let Some(s) = state_mut(hwnd) {
                    s.suppress_char = true;
                }
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                return;
            }
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

/// Consume a pending "this WM_CHAR belongs to a consumed key binding"
/// flag, so `WM_CHAR` dispatch can drop it.
fn take_suppressed_char(hwnd: HWND) -> bool {
    state_mut(hwnd).is_some_and(|s| std::mem::take(&mut s.suppress_char))
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
    // While search is active, typed text edits the query, not the shell.
    if let Some(search) = &mut s.search {
        if ch.is_control() {
            return;
        }
        search.query.push(ch);
        let tab = s.app.active_tab();
        search.refresh(tab);
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
        return;
    }
    let bytes = keyboard::encode_char(ch, mods);
    let _ = s.app.send_input_active(&bytes);
}

/// Ctrl+F: open the search overlay, or close it when already open
/// (SEARCH-002). Search starts with an empty query and no matches.
fn toggle_search(hwnd: HWND) {
    let Some(s) = state_mut(hwnd) else { return };
    if s.search.is_some() {
        s.search = None;
    } else {
        s.search = Some(SearchState::new());
    }
    s.suppress_char = true;
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Search-mode key handling (SEARCH-005/006/007/008/009). Returns `true`
/// when the key was consumed; printable keys are left to `WM_CHAR`, which
/// edits the query instead of sending bytes to the PTY.
fn handle_search_keydown(hwnd: HWND, vk: u16, mods: Modifiers) -> bool {
    if state_ref(hwnd).is_none_or(|s| s.search.is_none()) {
        return false;
    }
    let consumed = match vk {
        0x1B => {
            // Escape closes the search overlay.
            if let Some(s) = state_mut(hwnd) {
                s.search = None;
            }
            true
        }
        0x0D => {
            // Enter: next match.
            search_step(hwnd, 1);
            true
        }
        0x08 => {
            // Backspace: delete the last query character.
            search_backspace(hwnd);
            true
        }
        0x26 => {
            // Up arrow: previous match.
            search_step(hwnd, -1);
            true
        }
        0x28 => {
            // Down arrow: next match.
            search_step(hwnd, 1);
            true
        }
        0x43 if mods.alt => {
            // Alt+C: toggle case sensitivity.
            search_toggle(hwnd, SearchToggle::Case);
            true
        }
        0x57 if mods.alt => {
            // Alt+W: toggle whole-word matching.
            search_toggle(hwnd, SearchToggle::Word);
            true
        }
        0x52 if mods.alt => {
            // Alt+R: toggle regex matching.
            search_toggle(hwnd, SearchToggle::Regex);
            true
        }
        _ => false,
    };
    if consumed {
        if let Some(s) = state_mut(hwnd) {
            s.suppress_char = true;
        }
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    }
    consumed
}

enum SearchToggle {
    Case,
    Word,
    Regex,
}

fn search_toggle(hwnd: HWND, kind: SearchToggle) {
    let Some(s) = state_mut(hwnd) else { return };
    let Some(search) = &mut s.search else { return };
    let tab = s.app.active_tab();
    match kind {
        SearchToggle::Case => search.toggle_case(tab),
        SearchToggle::Word => search.toggle_whole_word(tab),
        SearchToggle::Regex => search.toggle_regex(tab),
    }
}

fn search_backspace(hwnd: HWND) {
    let Some(s) = state_mut(hwnd) else { return };
    let Some(search) = &mut s.search else { return };
    let tab = s.app.active_tab();
    search.backspace(tab);
}

/// Move to the next/previous match, focus its pane, and scroll its grid so
/// the match is visible (SEARCH-005/006). Wraps at the ends.
fn search_step(hwnd: HWND, delta: i32) {
    let Some(s) = state_mut(hwnd) else { return };
    let Some(search) = &mut s.search else { return };
    if search.matches.is_empty() {
        return;
    }
    let len = search.matches.len();
    let idx = if delta > 0 {
        (search.current + 1) % len
    } else {
        (search.current + len - 1) % len
    };
    search.current = idx;
    let match_ = search.matches[idx];
    s.app.focus_pane(match_.pane);
    if let Some(mut pane) = s
        .app
        .active_tab_mut()
        .and_then(|t| t.pane_at_mut(match_.pane))
    {
        pane.grid_mut().scroll_to_abs(match_.abs);
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// WM_MOUSEWHEEL: scroll the pane under the cursor through its
/// scrollback. The wheel never moves pane focus (SPLIT-004: the mouse
/// routes to what it points at).
fn handle_wheel(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
    let delta = (wparam.0 >> 16) as u16 as i16;
    let lines = if read_mods().ctrl {
        WHEEL_LINES * 2
    } else {
        WHEEL_LINES
    };
    // Wheel lparam carries *screen* coordinates; map them to the client.
    // The windows crate has no `GET_X_LPARAM`/`GET_Y_LPARAM` (C macros);
    // decode the signed 16-bit pair from the packed lparam.
    let mut pt = POINT {
        x: (lparam.0 & 0xFFFF) as u16 as i16 as i32,
        y: ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32,
    };
    let _ = unsafe { ScreenToClient(hwnd, &mut pt) };
    let (cell, proto, sgr, urxvt) = {
        let Some(s) = state_mut(hwnd) else { return };
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32;
        let pos = (
            px_to_dips(pt.x.max(0) as u32, dpi),
            px_to_dips(pt.y.max(0) as u32, dpi),
        );
        let cell = cell_at(s, pos.0, pos.1);
        let (proto, sgr, urxvt) = cell.map_or((MouseProtocol::Off, false, false), |(idx, _, _)| {
            s.app.active_tab().and_then(|t| t.pane_at(idx)).map_or(
                (MouseProtocol::Off, false, false),
                |p| {
                    (
                        p.parser().mouse_protocol,
                        p.parser().mouse_sgr_coordinates,
                        p.parser().mouse_urxvt_coordinates,
                    )
                },
            )
        });
        (cell, proto, sgr, urxvt)
    };
    if let Some((idx, row, col)) = cell
        && proto != MouseProtocol::Off
    {
        let bytes =
            mouse_report_bytes(if delta < 0 { 64 } else { 65 }, col, row, false, sgr, urxvt);
        if let Some(s) = state_mut(hwnd) {
            let _ = s.app.send_input_pane(idx, &bytes);
        }
        return;
    }
    if let Some((idx, _, _)) = cell
        && let Some(s) = state_mut(hwnd)
        && let Some(mut pane) = s.app.active_tab_mut().and_then(|t| t.pane_at_mut(idx))
    {
        let grid = pane.grid_mut();
        if delta < 0 {
            grid.scroll_up(lines);
        } else {
            grid.scroll_down(lines);
        }
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Client-area pointer position (from a mouse-message `lparam`) in DIPs.
/// The windows crate has no `GET_X_LPARAM`; decode the signed 16-bit
/// pair ourselves (the mouse coordinates fit in the low/high words).
fn cursor_dips(hwnd: HWND, lparam: LPARAM) -> (f32, f32) {
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32;
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (
        px_to_dips(x.max(0) as u32, dpi),
        px_to_dips(y.max(0) as u32, dpi),
    )
}

/// Client-area width in DIPs (tab-bar layout needs it for hit tests).
fn client_width_dips(hwnd: HWND) -> f32 {
    let mut r = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut r) };
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32;
    px_to_dips((r.right - r.left).max(1) as u32, dpi)
}

/// Grid cell under a client-area DIP position, addressed by pane:
/// `(pane flatten index, viewport row, viewport col)` inside that pane;
/// `None` for the tab bar. Clamped to the grid bounds so drags past the
/// edges extend the selection (WIN-004, SPLIT-004 hit-testing).
fn cell_at(s: &WindowState, x: f32, y: f32) -> Option<(usize, usize, usize)> {
    let bar = s.bar_height();
    if y < bar {
        return None;
    }
    let tab = s.app.active_tab()?;
    let col = (x / s.metrics.cell_width).floor().max(0.0) as usize;
    let row = ((y - bar) / s.metrics.cell_height).floor().max(0.0) as usize;
    let (cols, rows) = (tab.cols(), tab.rows());
    let col = col.min(cols.saturating_sub(1));
    let row = row.min(rows.saturating_sub(1));
    let idx = tab.layout().iter().position(|r| r.contains(col, row))?;
    let rect = tab.layout()[idx];
    Some((idx, row - rect.row, col - rect.col))
}

/// Viewport row -> absolute grid row inside pane `idx`, clamped to its
/// grid (panes tile the tab, so the mapping always exists).
fn abs_row_in_pane(s: &WindowState, idx: usize, row: usize) -> usize {
    s.app
        .active_tab()
        .and_then(|t| t.pane_at(idx)?.grid().abs_of_view(row))
        .unwrap_or(row)
}

/// WM_LBUTTONDOWN: tab bar hit (switch tab / press `+`) or focus the pane
/// under the pointer and start a text-selection drag anchored on the
/// clicked cell (WIN-004, SPLIT-003).
/// Mouse protocol active under the pointer (MOUSE-009..012).
fn mouse_protocol_at(s: &WindowState, idx: usize) -> MouseProtocol {
    s.app
        .active_tab()
        .and_then(|t| t.pane_at(idx))
        .map_or(MouseProtocol::Off, |p| p.parser().mouse_protocol)
}

/// Send a mouse report to pane `idx` if the pane requested reporting.
fn send_mouse_report(
    hwnd: HWND,
    idx: usize,
    row: usize,
    col: usize,
    button: u16,
    release: bool,
    drag: bool,
) {
    let (sgr, urxvt) = state_ref(hwnd).map_or((false, false), |s| {
        s.app
            .active_tab()
            .and_then(|t| t.pane_at(idx))
            .map_or((false, false), |p| {
                (
                    p.parser().mouse_sgr_coordinates,
                    p.parser().mouse_urxvt_coordinates,
                )
            })
    });
    let code = button.saturating_add(if drag { 32 } else { 0 });
    let bytes = mouse_report_bytes(code, col, row, release, sgr, urxvt);
    if let Some(s) = state_mut(hwnd) {
        let _ = s.app.send_input_pane(idx, &bytes);
    }
}

fn mouse_cell(hwnd: HWND, lparam: LPARAM) -> Option<(usize, usize, usize)> {
    let (x, y) = cursor_dips(hwnd, lparam);
    state_ref(hwnd).and_then(|s| cell_at(s, x, y))
}

fn handle_rbutton_down(hwnd: HWND, lparam: LPARAM) {
    if let Some((idx, row, col)) = mouse_cell(hwnd, lparam)
        && state_ref(hwnd).is_some_and(|s| mouse_protocol_at(s, idx) != MouseProtocol::Off)
    {
        send_mouse_report(hwnd, idx, row, col, 2, false, false);
        return;
    }
    handle_context_menu(hwnd, LPARAM(0));
}

fn handle_mbutton_down(hwnd: HWND, lparam: LPARAM) -> bool {
    if let Some((idx, row, col)) = mouse_cell(hwnd, lparam)
        && state_ref(hwnd).is_some_and(|s| mouse_protocol_at(s, idx) != MouseProtocol::Off)
    {
        send_mouse_report(hwnd, idx, row, col, 1, false, false);
        return true;
    }
    false
}

fn handle_context_menu(hwnd: HWND, lparam: LPARAM) {
    let mut pt = POINT {
        x: (lparam.0 & 0xFFFF) as u16 as i16 as i32,
        y: ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32,
    };
    if pt.x < 0 || pt.y < 0 {
        let _ = unsafe { GetCursorPos(&mut pt) };
    }
    unsafe {
        let menu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return,
        };
        let _ = append_menu_item(menu, "Copy	Ctrl+Shift+C", MENU_COPY);
        let _ = append_menu_item(menu, "Paste	Ctrl+Shift+V", MENU_PASTE);
        let _ = append_menu_separator(menu);
        let _ = append_menu_item(menu, "Search	Ctrl+F", MENU_SEARCH);
        let _ = append_menu_item(menu, "Split Right	Ctrl+Shift+Enter", MENU_SPLIT_RIGHT);
        let _ = append_menu_item(menu, "Split Below	Ctrl+Shift+-", MENU_SPLIT_DOWN);
        let _ = append_menu_separator(menu);
        let _ = append_menu_item(menu, "New Tab	Ctrl+Shift+T", MENU_NEW_TAB);
        let _ = append_menu_item(menu, "Close Tab	Ctrl+Shift+W", MENU_CLOSE_TAB);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = DestroyMenu(menu);
    }
}

fn handle_lbutton_down(hwnd: HWND, lparam: LPARAM) {
    let (x, y) = cursor_dips(hwnd, lparam);
    let bar = state_ref(hwnd).map_or(TAB_BAR_HEIGHT, |s| s.bar_height());
    if y < bar {
        let hit = state_ref(hwnd).map(|s| {
            let titles: Vec<String> = s.app.tabs().iter().map(|t| t.title().to_string()).collect();
            let layout = tabbar::layout(&titles, client_width_dips(hwnd), s.metrics.cell_width);
            tabbar::hit_test(&layout, x)
        });
        match hit {
            Some(Hit::Tab(i)) => {
                if let Some(s) = state_mut(hwnd) {
                    s.app.select_tab(i);
                    set_title(hwnd, &s.app);
                }
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            }
            Some(Hit::Plus) => new_tab(hwnd),
            Some(Hit::None) | None => {}
        }
        return;
    }
    let anchor = state_ref(hwnd).and_then(|s| {
        let (idx, row, col) = cell_at(s, x, y)?;
        let abs = abs_row_in_pane(s, idx, row);
        Some((Pos { row: abs, col }, idx))
    });
    let Some((a, idx)) = anchor else { return };
    // Clicking a pane focuses it (SPLIT-003); the keystrokes and the
    // follow-up selection drag then belong to that pane.
    if let Some(s) = state_mut(hwnd) {
        s.app.focus_pane(idx);
    }
    if state_ref(hwnd).is_some_and(|s| mouse_protocol_at(s, idx) != MouseProtocol::Off) {
        send_mouse_report(hwnd, idx, a.row, a.col, 0, false, false);
        return;
    }
    // Synthetic double-click detection: the system coalesces real mouse
    // clicks into WM_LBUTTONDBLCLK, but injected/synthesized input never
    // coalesces, so detect the second fast click on the same cell here.
    let tick = unsafe { GetTickCount64() } as i64;
    let dbl = state_ref(hwnd)
        .and_then(|s| s.last_click)
        .is_some_and(|(r, c, t)| {
            r == a.row && c == a.col && tick - t < unsafe { GetDoubleClickTime() } as i64
        });
    if let Some(s) = state_mut(hwnd) {
        s.last_click = Some((a.row, a.col, tick));
    }
    if dbl {
        select_word(hwnd, idx, a.row, a.col);
        return;
    }
    // Do NOT SetCapture here: taking capture on the button-down suppresses
    // the system's WM_LBUTTONDBLCLK coalescing. Capture is taken on the
    // first drag move instead. The selection stays empty until the drag
    // extends (a plain click must not leave a one-cell selection behind).
    if let Some(s) = state_mut(hwnd) {
        s.drag_anchor = Some((a, idx));
        if let Some(mut pane) = s.app.active_tab_mut().and_then(|t| t.pane_at_mut(idx)) {
            pane.set_selection(None);
        }
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// WM_LBUTTONDBLCLK: select the word under the pointer; the drag anchor
/// lets a follow-up drag extend the word selection (WIN-004).
fn handle_dblclick(hwnd: HWND, lparam: LPARAM) {
    let (x, y) = cursor_dips(hwnd, lparam);
    let cell = state_ref(hwnd).and_then(|s| {
        let (idx, row, col) = cell_at(s, x, y)?;
        let abs = abs_row_in_pane(s, idx, row);
        Some((idx, abs, col))
    });
    if let Some((idx, abs, col)) = cell {
        if let Some(s) = state_mut(hwnd) {
            s.last_click = Some((abs, col, unsafe { GetTickCount64() } as i64));
        }
        select_word(hwnd, idx, abs, col);
    }
}

/// Select the whole word at pane `idx`'s absolute grid position
/// (`abs`, `col`) (WIN-004).
fn select_word(hwnd: HWND, idx: usize, abs: usize, col: usize) {
    let word = state_ref(hwnd).and_then(|s| {
        let pane = s.app.active_tab()?.pane_at(idx)?;
        pane.grid().word_range_abs(abs, col)
    });
    let Some((lo, hi)) = word else { return };
    if let Some(s) = state_mut(hwnd) {
        s.drag_anchor = Some((Pos { row: abs, col: lo }, idx));
        if let Some(mut pane) = s.app.active_tab_mut().and_then(|t| t.pane_at_mut(idx)) {
            pane.set_selection(Some(Selection::between(
                Pos { row: abs, col: lo },
                Pos {
                    row: abs,
                    col: hi - 1,
                },
            )));
        }
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// WM_MOUSEMOVE with the left button down: extend the selection drag
/// inside the pane the drag started on (SPLIT-004: drags stay in their
/// pane; crossing a border clamps to that pane's edges).
fn handle_mouse_move(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
    const MK_LBUTTON: usize = 0x01;
    const MK_MBUTTON: usize = 0x10;
    const MK_RBUTTON: usize = 0x02;
    let (x, y) = cursor_dips(hwnd, lparam);
    if let Some((idx, row, col)) = state_ref(hwnd).and_then(|s| cell_at(s, x, y))
        && state_ref(hwnd).is_some_and(|s| mouse_protocol_at(s, idx) != MouseProtocol::Off)
    {
        let protocol = state_ref(hwnd).map_or(MouseProtocol::Off, |s| mouse_protocol_at(s, idx));
        let buttons = wparam.0;
        let button = if buttons & MK_LBUTTON != 0 {
            0
        } else if buttons & MK_MBUTTON != 0 {
            1
        } else if buttons & MK_RBUTTON != 0 {
            2
        } else {
            3
        };
        let press = button != 3;
        let drag = protocol == MouseProtocol::Drag || protocol == MouseProtocol::Any;
        if protocol == MouseProtocol::Any || (press && drag) {
            send_mouse_report(hwnd, idx, row, col, button, false, press);
        }
        return;
    }
    if wparam.0 & MK_LBUTTON == 0 {
        return;
    }
    let Some(s) = state_mut(hwnd) else { return };
    let Some((a, idx)) = s.drag_anchor else {
        return;
    };
    // Dragging over the tab bar clamps the selection to the top row of
    // the anchor pane.
    let Some((_, row, col)) = cell_at(s, x, y.max(s.bar_height())) else {
        return;
    };
    let row = row.min(
        s.app
            .active_tab()
            .and_then(|t| t.pane_at(idx))
            .map_or(row, |p| p.grid().rows().saturating_sub(1)),
    );
    let abs = abs_row_in_pane(s, idx, row);
    let cur = Pos { row: abs, col };
    // First drag move: take capture now (see handle_lbutton_down) so the
    // drag keeps delivering moves even past the window edges.
    unsafe { SetCapture(hwnd) };
    if let Some(mut pane) = s.app.active_tab_mut().and_then(|t| t.pane_at_mut(idx)) {
        pane.set_selection(Some(Selection::between(a, cur)));
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// WM_LBUTTONUP: end the selection drag (the selection stays visible
/// and copyable until the next click).
fn handle_lbutton_up(hwnd: HWND, lparam: LPARAM) {
    if let Some((idx, row, col)) = mouse_cell(hwnd, lparam)
        && state_ref(hwnd).is_some_and(|s| mouse_protocol_at(s, idx) != MouseProtocol::Off)
    {
        send_mouse_report(hwnd, idx, row, col, 0, true, false);
    }
    let _ = unsafe { ReleaseCapture() };
    if let Some(s) = state_mut(hwnd) {
        s.drag_anchor = None;
    }
}

/// WM_SETCURSOR: I-beam over the grid, arrow over the tab bar.
fn set_cursor_for_position(wparam: WPARAM) {
    let target = HWND(wparam.0 as _);
    let mut pt = POINT::default();
    if unsafe { GetCursorPos(&mut pt) }.is_err() {
        return;
    }
    let _ = unsafe { ScreenToClient(target, &mut pt) };
    let dpi = unsafe { GetDpiForWindow(target) }.max(96) as f32;
    let y = px_to_dips(pt.y.max(0) as u32, dpi);
    let bar = state_ref(target).map_or(TAB_BAR_HEIGHT, |s| s.bar_height());
    let idc = if y < bar { IDC_ARROW } else { IDC_IBEAM };
    if let Ok(cursor) = unsafe { LoadCursorW(None, idc) } {
        let _ = unsafe { SetCursor(Some(cursor)) };
    }
}

/// Ctrl+Shift+]/[: move the active tab by `delta` (wrapping).
fn switch_tab_rel(hwnd: HWND, delta: i32) {
    let Some(s) = state_mut(hwnd) else { return };
    let n = s.app.tab_count();
    if n < 2 {
        return;
    }
    let idx = ((s.app.active_index() as i32 + delta).rem_euclid(n as i32)) as usize;
    s.app.select_tab(idx);
    set_title(hwnd, &s.app);
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// F11: toggle borderless fullscreen over the nearest monitor,
/// restoring the saved window rect and styles on the second press.
fn toggle_fullscreen(hwnd: HWND) {
    let Some(s) = state_mut(hwnd) else { return };
    if let Some((rect, style, exstyle)) = s.fullscreen.take() {
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_STYLE, style);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, exstyle);
            let _ = SetWindowPos(
                hwnd,
                None,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOZORDER | SWP_FRAMECHANGED,
            );
        }
    } else {
        let mut rect = RECT::default();
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
        let exstyle = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        unsafe {
            let _ = GetWindowRect(hwnd, &mut rect);
            let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(mon, &mut mi).as_bool() {
                let area = mi.rcMonitor;
                // Keep WS_VISIBLE: a bare WS_POPUP style hides the window.
                SetWindowLongPtrW(hwnd, GWL_STYLE, (WS_POPUP | WS_VISIBLE).0 as isize);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    area.left,
                    area.top,
                    area.right - area.left,
                    area.bottom - area.top,
                    SWP_NOZORDER | SWP_FRAMECHANGED,
                );
            }
        }
        s.fullscreen = Some((rect, style, exstyle));
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// WM_TIMER: drain every shell, close dead panes (and the tabs they leave
/// behind), refresh the caption.
fn drain_and_cleanup(hwnd: HWND) {
    // Scope the GWLP_USERDATA borrow so it ends before sync_tab_bar re-enters.
    let changed = {
        let Some(s) = state_mut(hwnd) else { return };
        s.app.drain_shells();
        // Repaint only when a grid actually changed (NFR-113: no idle
        // repaints), and only the rows the grid wrote: paint the damage
        // directly instead of invalidating the client area, so a burst of
        // shell output costs one partial frame per tick (NFR-113/114).
        if s.app.take_dirty() {
            if let Some(r) = &mut s.renderer {
                r.paint_damage(&s.app, &s.theme, s.search.as_ref());
                s.app.clear_grid_damage();
            } else {
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            }
        }
        // Shell death closes its pane (SPLIT-006: the sibling takes the
        // box); a tab whose last pane dies closes with it, and the last
        // tab closing destroys the window.
        let mut quit = false;
        let mut closed = false;
        for idx in (0..s.app.tab_count()).rev() {
            // Collect with indices: `for_each_pane_ref` visits leaves in
            // flatten order, so the dead panes are closed from the far
            // end backwards and the live indices stay valid.
            let dead: Vec<usize> = s
                .app
                .tabs()
                .get(idx)
                .map(|tab| {
                    let mut out = Vec::new();
                    let mut i = 0usize;
                    tab.for_each_pane_ref(|pane: &Pane, _rect: Rect| {
                        if matches!(
                            pane.shell().state(),
                            crate::shell::ShellState::Inactive
                                | crate::shell::ShellState::Exited(_)
                        ) {
                            out.push(i);
                        }
                        i += 1;
                    });
                    out
                })
                .unwrap_or_default();
            for pane_idx in dead.into_iter().rev() {
                match s.app.close_tab_pane(idx, pane_idx) {
                    PaneClose::WindowQuit => {
                        quit = true;
                        break;
                    }
                    PaneClose::TabClosed => closed = true,
                    PaneClose::PaneClosed => closed = true,
                    PaneClose::NotFound => {}
                }
            }
            if quit {
                break;
            }
        }
        if quit {
            let _ = unsafe { DestroyWindow(hwnd) };
            return;
        }
        set_title(hwnd, &s.app);
        closed
    };
    if changed {
        // A vanished tab can hide the strip again (tab-bar = "auto").
        sync_tab_bar(hwnd);
    }
}

/// Ctrl+Shift+Enter / Ctrl+Shift+-: split the focused pane (SPLIT-005) and
/// spawn the tab's shell profile into the new pane at its box size
/// (SPLIT-007).
fn split_pane(hwnd: HWND, dir: Dir) {
    let split = state_mut(hwnd).is_some_and(|s| s.app.split_active(dir).is_some());
    if !split {
        return;
    }
    // The new pane takes half the tab: rebuild the tab bar strip (it may
    // appear with the pane's tab count), then spawn at the pane's box.
    sync_tab_bar(hwnd);
    let Some(s) = state_mut(hwnd) else { return };
    let (cols, rows) = s
        .app
        .active_tab()
        .map_or((s.spawn.cols, s.spawn.rows), |t| (t.cols(), t.rows()));
    let opts = profile_opts(s, cols.max(MIN_COLS), rows.max(MIN_ROWS));
    // `spawn_active` clamps cols/rows to the focused pane's own box.
    if let Err(e) = s.app.spawn_active(&opts) {
        eprintln!("rusttyterm: split shell spawn failed: {e}");
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Ctrl+Shift+W: close the focused pane when the tab is split (SPLIT-006,
/// with a confirm box when its shell is live); otherwise close the tab,
/// and closing the last tab closes the window (WM_DESTROY reclaims the
/// state and quits the loop).
fn close_pane_or_tab(hwnd: HWND) {
    let (multi, live) = match state_ref(hwnd) {
        Some(s) => (
            s.app.pane_count() > 1,
            s.app
                .active_tab()
                .is_some_and(|t| t.pane().shell().state() == crate::shell::ShellState::Running),
        ),
        None => return,
    };
    if multi && live {
        let text = wstr("Close the focused pane? Its shell is still running.");
        let caption = wstr("rusttyterm");
        let answer = unsafe {
            MessageBoxW(
                Some(hwnd),
                PCWSTR::from_raw(text.as_ptr()),
                PCWSTR::from_raw(caption.as_ptr()),
                MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
            )
        };
        if answer != IDYES {
            return;
        }
    }
    if multi {
        let quit = state_mut(hwnd).is_some_and(|st| st.app.close_active_pane());
        if quit {
            let _ = unsafe { DestroyWindow(hwnd) };
            return;
        }
    } else {
        let destroy = state_mut(hwnd).is_some_and(|s| s.app.close_tab(s.app.active_index()));
        if destroy {
            let _ = unsafe { DestroyWindow(hwnd) };
            return;
        }
    }
    // A vanished tab can hide the strip again (tab-bar = "auto").
    sync_tab_bar(hwnd);
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Ctrl+Shift+T: append a tab and spawn a shell at the current grid size.
fn new_tab(hwnd: HWND) {
    new_tab_with_title(hwnd, None);
}

fn new_tab_with_title(hwnd: HWND, title: Option<&str>) {
    if let Some(s) = state_mut(hwnd) {
        s.app.add_tab();
    }
    // Auto/hidden tab bars change the strip height when the count changes;
    // sync first so the new shell spawns at the grid the bar leaves room for.
    sync_tab_bar(hwnd);
    let Some(s) = state_mut(hwnd) else { return };
    let (cols, rows) = s
        .renderer
        .as_ref()
        .map_or((s.spawn.cols, s.spawn.rows), |r| r.grid_size());
    let opts = profile_opts(s, cols.max(MIN_COLS), rows.max(MIN_ROWS));
    if let Err(e) = s.app.spawn_active(&opts) {
        eprintln!("rusttyterm: shell spawn failed: {e}");
    }
    if let Some(title) = title
        && let Some(tab) = s.app.active_tab_mut()
    {
        tab.set_title(title);
    }
    set_title(hwnd, &s.app);
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Ctrl+Shift+C: copy the mouse selection; the whole visible viewport
/// when nothing is selected (WIN-004).
fn copy_screen(hwnd: HWND) {
    let Some(s) = state_ref(hwnd) else { return };
    let Some(tab) = s.app.active_tab() else {
        return;
    };
    let text = tab
        .pane()
        .selection_text()
        .unwrap_or_else(|| Renderer::visible_text(tab.pane().grid()));
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

/// Client-area pixel size for a `cols` x `rows` grid plus the tab bar.
fn px_size(metrics: CellMetrics, cols: usize, rows: usize, bar_height: f32) -> (i32, i32) {
    let w = (cols.max(1) as f32 * metrics.cell_width).ceil() as i32;
    let h = (rows.max(1) as f32 * metrics.cell_height + bar_height).ceil() as i32;
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
    let font = cfg.font();
    let metrics = measure_cells(dpi, &font)?;
    let cols = cfg.window.cols.max(MIN_COLS);
    let rows = cfg.window.rows.max(MIN_ROWS);
    // One tab at startup: "auto"/"hidden" bars start collapsed (TAB-010).
    let bar = match cfg.tab_bar {
        TabBar::Always => TAB_BAR_HEIGHT,
        TabBar::Auto | TabBar::Hidden => 0.0,
    };
    let (cell_w, cell_h) = px_size(metrics, cols, rows, bar);
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: cell_w,
        bottom: cell_h,
    };
    let style = WS_OVERLAPPEDWINDOW | WS_VISIBLE;
    unsafe { AdjustWindowRect(&mut rect, style, false) }?;

    let state = Box::new(WindowState {
        app: App::with_size(cols, rows, cfg.scrollback),
        theme,
        spawn,
        font: font.clone(),
        tab_bar: cfg.tab_bar,
        renderer: None,
        metrics,
        timer_id: 0,
        drag_anchor: None,
        last_click: None,
        suppress_char: false,
        fullscreen: None,
        search: None,
        profile: 0,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_generated_command_ranges_stay_disjoint() {
        assert_eq!(theme_command(MENU_THEME_BASE + 1), Some(1));
        assert_eq!(theme_command(MENU_THEME_BASE + theme::PRESETS.len()), None);
        assert_eq!(profile_command(MENU_PROFILE_BASE), Some(0));
        assert_eq!(profile_command(MENU_THEME_BASE), None);
    }
}
