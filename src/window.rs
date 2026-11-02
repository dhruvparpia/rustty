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
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, EndPaint, GetMonitorInfoW, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MonitorFromPoint, MonitorFromWindow, PAINTSTRUCT, ScreenToClient,
};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::UI::Accessibility::{
    IRawElementProviderSimple, UiaReturnRawElementProvider, UiaRootObjectId,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetDpiForWindow,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::Ime::{
    CFS_POINT, COMPOSITIONFORM, CPS_COMPLETE, GCS_RESULTSTR, ImmGetCompositionStringW,
    ImmGetContext, ImmNotifyIME, ImmReleaseContext, ImmSetCompositionWindow, NI_COMPOSITIONSTR,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetKeyState, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey,
    ReleaseCapture, SetCapture, SetFocus, UnregisterHotKey, VK_CONTROL, VK_LCONTROL, VK_LMENU,
    VK_RMENU, VK_SHIFT,
};
use windows::Win32::UI::Shell::{
    DragAcceptFiles, DragFinish, DragQueryFileW, DragQueryPoint, HDROP,
    SetCurrentProcessExplicitAppUserModelID, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, AppendMenuW, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CREATESTRUCTW, CS_HREDRAW,
    CS_VREDRAW, CW_USEDEFAULT, CheckMenuItem, CreateMenu, CreatePopupMenu, CreateWindowExW,
    DLGPROC, DLGTEMPLATE, DS_MODALFRAME, DefWindowProcW, DeleteMenu, DestroyMenu, DestroyWindow,
    DialogBoxIndirectParamW, DispatchMessageW, EnableMenuItem, EndDialog, GW_CHILD, GW_HWNDNEXT,
    GWL_EXSTYLE, GWL_STYLE, GWLP_USERDATA, GetClientRect, GetCursorPos, GetForegroundWindow,
    GetMenu, GetMenuItemCount, GetMessageW, GetSubMenu, GetWindow, GetWindowLongPtrW,
    GetWindowRect, GetWindowTextW, HMENU, HTCLIENT, HWND_TOPMOST, IDC_ARROW, IDC_IBEAM, IDCANCEL,
    IDOK, IDYES, IsWindow, IsWindowVisible, IsZoomed, KillTimer, LWA_ALPHA, LoadCursorW, LoadIconW,
    MB_DEFBUTTON2, MB_ICONASTERISK, MB_ICONWARNING, MB_OK, MB_YESNO, MF_BYCOMMAND, MF_BYPOSITION,
    MF_CHECKED, MF_ENABLED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MINMAXINFO,
    MSG, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW, SIZE_MINIMIZED, SW_HIDE,
    SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOZORDER,
    SetCursor, SetForegroundWindow, SetLayeredWindowAttributes, SetMenu, SetTimer,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TPM_LEFTALIGN, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_CHAR, WM_CLOSE,
    WM_COMMAND, WM_CONTEXTMENU, WM_COPY, WM_CREATE, WM_DESTROY, WM_DROPFILES, WM_ENTERSIZEMOVE,
    WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_GETOBJECT, WM_HOTKEY, WM_IME_CHAR,
    WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION, WM_INITDIALOG,
    WM_INITMENUPOPUP, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NULL, WM_PAINT, WM_PASTE, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SETCURSOR, WM_SETFOCUS, WM_SIZE, WM_SYSCHAR, WM_SYSKEYDOWN, WM_TIMER,
    WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_OVERLAPPEDWINDOW, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{ComObject, PCWSTR, Result};

use crate::app::{App, PaneClose};
use crate::clipboard;
use crate::color::Color;
use crate::config::{
    self, Config, FontConfig, KeyAction, QuickEdge, QuickTerminalConfig, TabBar, TabBarPlacement,
};
use crate::discovery::{self, Found};
use crate::keyboard::{self, Modifiers};
use crate::palette::{Command, PaletteItem, PaletteState, builtin_shortcut, display_shortcut};
use crate::pane::Pane;
use crate::parser::{MouseProtocol, bracketed_paste_bytes, mouse_report_bytes};
use crate::render::{
    CellMetrics, Renderer, SEARCH_BAR_HEIGHT, dips_to_px, measure_cells, palette_row_height,
    px_to_dips,
};
use crate::search::SearchState;
use crate::selection::{Pos, Selection};
use crate::shell::{ShellFamily, SpawnOptions, quote_for_drop, shell_family};
use crate::split::{Dir, Rect, Side};
use crate::tabbar::TAB_BAR_HEIGHT;
use crate::theme::{self, Theme};
use crate::uia::{DipRect, Provider};

/// Window class name.
const CLASS_NAME: &str = "rustty";

/// App display name used in the window caption.
const APP_NAME: &str = "rustty";

/// Shell-output poll period in milliseconds.
const POLL_MS: u32 = 33;

/// Timer id for the shell-output poll.
const POLL_TIMER: usize = 1;

/// Timer id for the cursor blink (CONFIG-009).
const BLINK_TIMER: usize = 2;

/// The AUMID that owns our toasts (PKG-002: Windows attributes them to the app).
fn aumid() -> windows::core::HSTRING {
    windows::core::HSTRING::from("com.dhruv.rustty")
}
/// BELL-003/004: how long one visual-bell flash lasts.
const BELL_MS: u32 = 300;
/// BELL-003/004: the visual-bell timer id (`SetTimer` `nIDEvent`).
const BELL_TIMER: usize = 3;
/// Signboard tab-title marquee (TAB-013 hover scroll): timer id and cadence.
const MARQUEE_TIMER: usize = 4;
const MARQUEE_MS: u32 = 50;
/// Issue #42: incremental search scan timer id and cadence (a bounded
/// scan chunk runs per tick, keeping typing and scrolling responsive).
const SEARCH_TIMER: usize = 5;
const SEARCH_SCAN_MS: u32 = 33;
/// Typical terminal cursor blink period.
const BLINK_MS: u32 = 530;

/// Lines scrolled per mouse-wheel notch.
const WHEEL_LINES: usize = 3;

/// Smallest cell count the window can be resized to.
const MIN_COLS: usize = 10;
/// Smallest cell count the window can be resized to.
const MIN_ROWS: usize = 4;

/// `RegisterHotKey` id of the quick-terminal summon key (the main window
/// owns the registration; WM_HOTKEY arrives there).
const QUICK_HOTKEY_ID: i32 = 0xE900;

/// The quick-terminal window handle (roadmap v0.2 "Hotkey summon from
/// screen edge"), 0 when there is none. One per process; the main window
/// owns the message loop that creates and destroys it.
static QUICK_HWND: AtomicIsize = AtomicIsize::new(0);

/// Recipe for (re)creating the quick window: config, theme and spawn
/// options as the main window had them at startup.
static QUICK_SPEC: Mutex<Option<(Config, theme::Theme, SpawnOptions)>> = Mutex::new(None);

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
const MENU_PREV_COMMAND: usize = 0xE823;
const MENU_NEXT_COMMAND: usize = 0xE824;
const MENU_COPY_LAST_CMD: usize = 0xE825;
const MENU_NEXT_TAB: usize = 0xE831;
const MENU_PREV_TAB: usize = 0xE832;
const MENU_MINIMIZE: usize = 0xE833;
const MENU_MAXIMIZE: usize = 0xE834;
const MENU_CLOSE_WINDOW: usize = 0xE835;
const MENU_RENAME_TAB: usize = 0xE836;
const MENU_TAB_LEFT: usize = 0xE837;
const MENU_TAB_RIGHT: usize = 0xE838;
const MENU_ABOUT: usize = 0xE841;
const MENU_THEME_BASE: usize = 0xF000;
const MENU_PROFILE_BASE: usize = 0xF100;
const MENU_WS_SAVE_AS: usize = 0xE851;
const MENU_WS_OVERWRITE: usize = 0xE852;
const MENU_WS_RENAME: usize = 0xE853;
/// Command palette (issue #39).
const MENU_PALETTE: usize = 0xE854;
/// OSC 8 link under the pointer: open / copy target (issue #41).
const MENU_OPEN_LINK: usize = 0xE855;
const MENU_COPY_LINK: usize = 0xE856;
/// Session-view tab menu entries (issue #49).
const MENU_VIEW_COPY_ATTACH: usize = 0xE860;
const MENU_VIEW_TAKE_CONTROL: usize = 0xE861;
const MENU_VIEW_TERMINATE: usize = 0xE862;
/// Workspaces submenu: "Open <name>" items (WORKSPACE-003), one per
/// template, capped by the store's own 64-template limit.
const MENU_WS_OPEN_BASE: usize = 0xF200;
/// Workspaces submenu: "Delete <name>" items, one per template.
const MENU_WS_DEL_BASE: usize = 0xF300;

/// NUL-terminated UTF-16 for `PCWSTR` parameters.
fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Ctrl/Alt/Shift state read from `GetKeyState`.
fn read_mods() -> Modifiers {
    unsafe {
        // Issue #24: AltGr is a right-Alt that many layouts use to type
        // characters (|, @, \). Treat right-Alt as plain Alt only when
        // left-Ctrl is not also down, which is how AltGr is signalled.
        let lmenu = GetKeyState(VK_LMENU.0 as i32) < 0;
        let rmenu = GetKeyState(VK_RMENU.0 as i32) < 0;
        let lctrl = GetKeyState(VK_LCONTROL.0 as i32) < 0;
        Modifiers {
            ctrl: GetKeyState(VK_CONTROL.0 as i32) < 0,
            alt: lmenu || (rmenu && !lctrl),
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

const PROFILES: [BuiltinProfile; 7] = [
    BuiltinProfile {
        name: "PowerShell",
        program: "powershell.exe",
        args: &[],
    },
    BuiltinProfile {
        name: "Rust Console",
        program: "powershell.exe",
        args: &[
            "-NoExit",
            "-Command",
            "& 'C:\\Program Files (x86)\\Microsoft Visual Studio\\2022\\BuildTools\\Common7\\Tools\\Launch-VsDevShell.ps1' -Arch amd64 -SkipAutomaticLocation; Set-Location $HOME",
        ],
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
fn build_menu(profiles: &[ProfileSpec], theme_names: &[String]) -> Result<HMENU> {
    unsafe {
        let menu = CreateMenu()?;

        let file = CreatePopupMenu()?;
        append_menu_item(file, "&New Tab\tCtrl+Shift+T", MENU_NEW_TAB)?;
        append_menu_item(file, "Close Pane/&Tab\tCtrl+Shift+W", MENU_CLOSE_TAB)?;
        append_menu_separator(file)?;
        append_menu_item(file, "&Split Right\tCtrl+Shift+Enter", MENU_SPLIT_RIGHT)?;
        append_menu_item(file, "Split &Below\tCtrl+Shift+-", MENU_SPLIT_DOWN)?;
        append_menu_separator(file)?;
        let profile_menu = CreatePopupMenu()?;
        for (idx, profile) in profiles.iter().enumerate() {
            append_menu_item(
                profile_menu,
                &format!("&{}", profile.name),
                MENU_PROFILE_BASE + idx,
            )?;
        }
        append_menu_submenu(file, "&Profiles", profile_menu)?;
        append_menu_separator(file)?;
        // WORKSPACE-003: rebuilt from workspaces.toml every time it opens
        // (update_menu_checks); starts empty.
        let ws_menu = CreatePopupMenu()?;
        append_menu_submenu(file, "W&orkspaces", ws_menu)?;
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
        append_menu_item(view, "Previous Command\tCtrl+Shift+Up", MENU_PREV_COMMAND)?;
        append_menu_item(view, "Next Command\tCtrl+Shift+Down", MENU_NEXT_COMMAND)?;
        append_menu_item(view, "&Copy Last Command Output", MENU_COPY_LAST_CMD)?;
        append_menu_item(view, "Command &Palette\tCtrl+Shift+P", MENU_PALETTE)?;
        append_menu_separator(view)?;
        append_menu_item(view, "Split &Right\tCtrl+Shift+Enter", MENU_SPLIT_RIGHT)?;
        append_menu_item(view, "Split Be&low\tCtrl+Shift+-", MENU_SPLIT_DOWN)?;
        append_menu_separator(view)?;
        append_menu_item(view, "Full &Screen\tF11", MENU_FULLSCREEN)?;
        append_menu_separator(view)?;
        let themes = CreatePopupMenu()?;
        for (idx, name) in theme_names.iter().enumerate() {
            append_menu_item(themes, &format!("&{name}"), MENU_THEME_BASE + idx)?;
        }
        append_menu_submenu(view, "&Themes", themes)?;
        append_menu_submenu(menu, "&View", view)?;

        let window = CreatePopupMenu()?;
        append_menu_item(window, "&Next Tab\tCtrl+Shift+]", MENU_NEXT_TAB)?;
        append_menu_item(window, "&Previous Tab\tCtrl+Shift+[", MENU_PREV_TAB)?;
        append_menu_item(window, "Move Tab &Left\tCtrl+Shift+PgUp", MENU_TAB_LEFT)?;
        append_menu_item(window, "Move Tab &Right\tCtrl+Shift+PgDn", MENU_TAB_RIGHT)?;
        append_menu_item(window, "&Rename Tab\tCtrl+Shift+R", MENU_RENAME_TAB)?;
        append_menu_separator(window)?;
        append_menu_item(window, "&Minimize", MENU_MINIMIZE)?;
        append_menu_item(window, "Maxi&mize / Restore", MENU_MAXIMIZE)?;
        append_menu_separator(window)?;
        append_menu_item(window, "&Close Window", MENU_CLOSE_WINDOW)?;
        append_menu_submenu(menu, "&Window", window)?;

        let help = CreatePopupMenu()?;
        append_menu_item(help, "&About rustty", MENU_ABOUT)?;
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

/// One entry of the profile selector: the built-in shells merged with the
/// `[[profile]]` entries from config (SHELL-017..022, TAB-015, THEME-011).
#[derive(Clone)]
struct ProfileSpec {
    name: String,
    program: String,
    args: Vec<String>,
    cwd: Option<String>,
    env: std::collections::BTreeMap<String, String>,
    theme: Option<String>,
    color: Option<Color>,
    scrollback: Option<usize>,
    /// SHELL-019: profile font family / size / line height.
    font: Option<String>,
    font_size: Option<f32>,
    line_height: Option<f32>,
    /// TAB-018: ask before this tab's shells are closed.
    confirm_close: bool,
}

impl ProfileSpec {
    fn builtin(profile: &BuiltinProfile) -> Self {
        Self {
            name: profile.name.to_string(),
            program: profile.program.to_string(),
            args: profile.args.iter().map(|arg| (*arg).to_string()).collect(),
            cwd: None,
            env: std::collections::BTreeMap::new(),
            theme: None,
            color: None,
            scrollback: None,
            font: None,
            font_size: None,
            line_height: None,
            confirm_close: true,
        }
    }

    /// Issue #40: a discovered shell becomes a plain profile entry (no theme,
    /// colour, font or cwd overrides; a config profile can add them by name).
    fn found(found: &Found) -> Self {
        Self {
            name: found.name.clone(),
            program: found.program.clone(),
            args: found.args.clone(),
            cwd: None,
            env: std::collections::BTreeMap::new(),
            theme: None,
            color: None,
            scrollback: None,
            font: None,
            font_size: None,
            line_height: None,
            confirm_close: true,
        }
    }
}

/// Snapshot of the last discovery pass (the global keeps the results of the
/// worker thread; the window merges them at startup and on WM_APP).
fn discovered_now() -> Vec<Found> {
    discovery::DISCOVERED
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

/// Run the discovery probes on a worker thread (DISCOVERY-006: startup never
/// waits for them) and notify `hwnd` when results are ready. Every probe
/// failure contributes an empty list; the window just keeps its current
/// profiles. Refresh = Reload Config re-runs this.
fn start_discovery(hwnd: HWND) {
    // HWND is not Send; carry the handle as an isize and rebuild it in
    // the worker. Posting WM_APP to a dead window is a harmless no-op.
    let hwnd_arg = hwnd.0 as isize;
    std::thread::spawn(move || {
        let found = discovery::discover();
        if let Ok(mut guard) = discovery::DISCOVERED.lock() {
            *guard = found;
        }
        let target = HWND(hwnd_arg as *mut core::ffi::c_void);
        let _ = unsafe { PostMessageW(Some(target), WM_APP, WPARAM(0), LPARAM(0)) };
    });
}

/// Built-in shells, then discovered shells (issue #40), then config
/// profiles: a config profile that uses an earlier name overrides it, and a
/// discovered entry never replaces a config profile (config > discovered >
/// builtins, so user overrides win and custom profiles are never overwritten).
fn merge_profiles(cfg: &Config, discovered: &[discovery::Found]) -> Vec<ProfileSpec> {
    let mut merged: Vec<ProfileSpec> = PROFILES.iter().map(ProfileSpec::builtin).collect();
    for found in discovered {
        if cfg.profiles.iter().any(|p| p.name == found.name) {
            continue;
        }
        let spec = ProfileSpec::found(found);
        match merged.iter_mut().position(|e| e.name == spec.name) {
            Some(idx) => merged[idx] = spec,
            None => merged.push(spec),
        }
    }
    for profile in &cfg.profiles {
        let spec = ProfileSpec {
            name: profile.name.clone(),
            program: profile.command.clone(),
            args: profile.args.clone(),
            cwd: Some(config::expand_path(&profile.starting_directory)),
            env: profile.env.clone(),
            theme: (!profile.theme.is_empty()).then(|| profile.theme.clone()),
            color: profile.tab_color.as_deref().and_then(Color::parse_hex),
            scrollback: profile.scrollback,
            font: profile
                .font
                .clone()
                .filter(|family| !family.trim().is_empty()),
            font_size: profile.font_size,
            line_height: profile.line_height,
            confirm_close: profile.confirm_close.unwrap_or(true),
        };
        match merged.iter_mut().position(|e| e.name == spec.name) {
            Some(idx) => merged[idx] = spec,
            None => merged.push(spec),
        }
    }
    merged
}

fn active_profile(s: &WindowState) -> ProfileSpec {
    s.profiles[s.profile.min(s.profiles.len() - 1)].clone()
}

/// Spawn options for the selected profile at the given grid size, including
/// its starting directory and environment (SHELL-019, SHELL-021).
fn profile_opts(s: &WindowState, cols: usize, rows: usize) -> SpawnOptions {
    let profile = active_profile(s);
    let mut opts = SpawnOptions {
        program: profile.program,
        args: profile.args,
        cols,
        rows,
        cwd: profile.cwd,
        env: profile.env,
    };
    if s.cfg.shell_integration {
        opts.apply_shell_integration();
    }
    opts
}

/// Contents of the View→Themes submenu: the built-in presets, then the
/// theme files in the theme directory (roadmap v0.2 "preset browser").
fn menu_theme_names(theme_dir: Option<&std::path::Path>) -> Vec<String> {
    theme::PRESETS
        .iter()
        .map(|name| (*name).to_string())
        .chain(theme::list_files(theme_dir))
        .collect()
}

fn active_theme_index(s: &WindowState) -> usize {
    menu_theme_names(s.theme_dir.as_deref())
        .iter()
        .position(|name| *name == s.theme.name)
        .unwrap_or(usize::MAX)
}

fn theme_command(id: usize) -> Option<usize> {
    id.checked_sub(MENU_THEME_BASE)
}

fn profile_command(s: &WindowState, id: usize) -> Option<usize> {
    id.checked_sub(MENU_PROFILE_BASE)
        .filter(|idx| *idx < s.profiles.len())
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
            for idx in 0..state.profiles.len() {
                check_menu_item(profiles, MENU_PROFILE_BASE + idx, state.profile == idx);
            }
        }
        if let Some(ws) = submenu_of(file, 8) {
            rebuild_workspaces_menu(ws, state);
        }
    }

    if let Some(view) = submenu_of(menu, 2)
        && same_menu(popup, view)
    {
        if let Some(themes) = submenu_of(view, 7) {
            let active = active_theme_index(state);
            for idx in 0..menu_theme_names(state.theme_dir.as_deref()).len() {
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
    // WORKSPACE-003: template ids route first — theme_command is
    // unbounded (id - 0xF000) and would swallow 0xF200+/0xF300+, and
    // profile ids share the 0xF1xx neighbourhood.
    if let Some(idx) = id
        .checked_sub(MENU_WS_OPEN_BASE)
        .filter(|idx| *idx < crate::workspace::MAX_WORKSPACES)
    {
        open_workspace_by_index(hwnd, idx);
        return;
    }
    if let Some(idx) = id
        .checked_sub(MENU_WS_DEL_BASE)
        .filter(|idx| *idx < crate::workspace::MAX_WORKSPACES)
    {
        delete_workspace_by_index(hwnd, idx);
        return;
    }
    match id {
        MENU_WS_SAVE_AS => {
            save_workspace_as(hwnd);
            return;
        }
        MENU_WS_OVERWRITE => {
            overwrite_workspace(hwnd);
            return;
        }
        MENU_WS_RENAME => {
            rename_workspace(hwnd);
            return;
        }
        _ => {}
    }
    // Profiles next: theme_command is unbounded (id - 0xF000), so profile
    // ids (0xF100+) would otherwise be swallowed as out-of-range themes.
    if let Some(idx) = state_ref(hwnd).and_then(|s| profile_command(s, id)) {
        open_profile_tab(hwnd, idx);
        return;
    }
    if let Some(idx) = theme_command(id) {
        set_theme(hwnd, idx);
        return;
    }
    match id {
        MENU_NEW_TAB => new_tab(hwnd),
        MENU_CLOSE_TAB => close_pane_or_tab(hwnd),
        MENU_SPLIT_RIGHT => split_pane(hwnd, Dir::Vertical),
        MENU_SPLIT_DOWN => split_pane(hwnd, Dir::Horizontal),
        MENU_SEARCH => toggle_search(hwnd),
        MENU_PALETTE => toggle_palette(hwnd),
        MENU_PREV_COMMAND => command_jump(hwnd, -1),
        MENU_NEXT_COMMAND => command_jump(hwnd, 1),
        MENU_COPY_LAST_CMD => copy_last_command(hwnd),
        MENU_VIEW_COPY_ATTACH => copy_attach_command(hwnd),
        MENU_VIEW_TAKE_CONTROL => take_view_control(hwnd),
        MENU_VIEW_TERMINATE => terminate_view_session(hwnd),
        MENU_FULLSCREEN => toggle_fullscreen(hwnd),
        MENU_COPY => copy_screen(hwnd),
        MENU_PASTE => paste_clipboard(hwnd),
        MENU_OPEN_LINK => {
            if let Some(uri) = state_mut(hwnd).and_then(|s| s.link_target.take()) {
                open_link(hwnd, &uri);
            }
        }
        MENU_COPY_LINK => {
            if let Some(uri) = state_mut(hwnd).and_then(|s| s.link_target.take()) {
                let _ = clipboard::set_text(&uri);
            }
        }
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
        MENU_RENAME_TAB => start_rename_tab(hwnd),
        MENU_TAB_LEFT => move_active_tab(hwnd, -1),
        MENU_TAB_RIGHT => move_active_tab(hwnd, 1),
        MENU_ABOUT => show_about(hwnd),
        _ => {}
    }
}

fn set_theme(hwnd: HWND, idx: usize) {
    {
        let Some(s) = state_mut(hwnd) else {
            return;
        };
        let Some(name) = menu_theme_names(s.theme_dir.as_deref()).get(idx).cloned() else {
            return;
        };
        // Preset by name, then theme file, then predawn (THEME-014). A theme
        // file that fails to parse is reported; the switch is not fatal.
        let (theme, problems) =
            theme::resolve_configured(&name, &theme::ThemeFile::default(), s.theme_dir.as_deref());
        for problem in problems {
            eprintln!("rustty: {problem}");
        }
        s.theme = theme;
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

fn open_profile_tab(hwnd: HWND, idx: usize) {
    let Some(s) = state_mut(hwnd) else {
        return;
    };
    let Some(spec) = s.profiles.get(idx).cloned() else {
        return;
    };
    s.profile = idx;
    new_tab_with_profile(hwnd, spec);
}

fn open_preferences(hwnd: HWND) {
    let Some(path) = config::config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if !path.exists() {
        let _ = std::fs::write(&path, b"# rustty config\n");
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
        "rustty {}\n\nA native Windows terminal emulator.\nMIT licensed. No telemetry.",
        env!("CARGO_PKG_VERSION")
    ));
    let caption = wstr("About rustty");
    unsafe {
        let _ = MessageBoxW(
            Some(hwnd),
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_OK,
        );
    }
}

// ---- workspace templates (WORKSPACE-003..008) ---------------------------

/// Menu labels eat `&` as mnemonics: template names carry them literally.
fn ws_menu_label(name: &str) -> String {
    name.replace('&', "&&")
}

fn ws_report(hwnd: HWND, text: &str) {
    let text = wstr(text);
    let caption = wstr("rustty");
    unsafe {
        let _ = MessageBoxW(
            Some(hwnd),
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_OK | MB_ICONWARNING,
        );
    }
}

fn ws_confirm(hwnd: HWND, text: &str, caption: &str) -> bool {
    let text = wstr(text);
    let caption = wstr(caption);
    unsafe {
        MessageBoxW(
            Some(hwnd),
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_YESNO | MB_DEFBUTTON2,
        ) == IDYES
    }
}

/// WORKSPACE-003: rebuild the Workspaces submenu every time it opens:
/// one "Open" per template, the save/overwrite/rename commands, and a
/// per-template Delete submenu. The store is re-read, so hand edits show
/// up immediately.
fn rebuild_workspaces_menu(ws: HMENU, state: &WindowState) {
    let names = crate::workspace::names();
    unsafe {
        while GetMenuItemCount(Some(ws)) > 0 {
            let _ = DeleteMenu(ws, 0, MF_BYPOSITION);
        }
        for (i, name) in names.iter().enumerate() {
            let _ = append_menu_item(
                ws,
                &format!("&Open \"{}\"", ws_menu_label(name)),
                MENU_WS_OPEN_BASE + i,
            );
        }
        if !names.is_empty() {
            let _ = append_menu_separator(ws);
        }
        let _ = append_menu_item(ws, "Save Workspace &As...", MENU_WS_SAVE_AS);
        let current = state.ws_source.as_deref().unwrap_or("");
        let _ = append_menu_item(
            ws,
            &format!("&Overwrite \"{}\"", ws_menu_label(current)),
            MENU_WS_OVERWRITE,
        );
        enable_menu_item(ws, MENU_WS_OVERWRITE, state.ws_source.is_some());
        let _ = append_menu_item(ws, "Re&name Workspace...", MENU_WS_RENAME);
        enable_menu_item(ws, MENU_WS_RENAME, state.ws_source.is_some());
        if !names.is_empty() {
            let _ = append_menu_separator(ws);
            if let Ok(del) = CreatePopupMenu() {
                for (i, name) in names.iter().enumerate() {
                    let _ = append_menu_item(del, &ws_menu_label(name), MENU_WS_DEL_BASE + i);
                }
                let _ = append_menu_submenu(ws, "&Delete Workspace", del);
            }
        }
    }
}

fn open_workspace_by_index(hwnd: HWND, idx: usize) {
    let Some(name) = crate::workspace::names().get(idx).cloned() else {
        return;
    };
    open_workspace_named(hwnd, &name);
}

/// WORKSPACE-008: opening a template replaces this window's tabs (with a
/// confirm, since every running shell is closed).
fn open_workspace_named(hwnd: HWND, name: &str) {
    let Some(path) = crate::workspace::store_path() else {
        return;
    };
    let Some(ws) = crate::workspace::workspace_at(&path, name) else {
        ws_report(hwnd, &format!("No workspace template named {name:?}."));
        return;
    };
    let question = format!(
        "Open workspace {:?}?\n\nEvery open tab in this window will be closed.",
        ws.name
    );
    if !ws_confirm(hwnd, &question, "Open workspace") {
        return;
    }
    {
        let Some(s) = state_mut(hwnd) else {
            return;
        };
        while s.app.tab_count() > 0 {
            s.app.close_tab(0);
        }
        s.ws_source = Some(ws.name.clone());
        s.ws_pending = Some(ws_leaf_cmds(&ws));
        s.restore = Some(ws.as_session());
    }
    let Some(s) = state_mut(hwnd) else {
        return;
    };
    restore_tabs(s);
    sync_tab_bar(hwnd);
    if let Some(s) = state_mut(hwnd) {
        spawn_restored(s);
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

fn save_workspace_as(hwnd: HWND) {
    let Some(path) = crate::workspace::store_path() else {
        return;
    };
    let initial = state_ref(hwnd)
        .and_then(|s| s.ws_source.clone())
        .unwrap_or_default();
    let Some(name) = prompt_workspace_name(hwnd, "Save workspace as", &initial) else {
        return;
    };
    let Some(s) = state_ref(hwnd) else {
        return;
    };
    // WORKSPACE-006: the UI captures layout, directories, titles and
    // focus — never the commands panes happen to be running.
    let session = snapshot_session(s, hwnd);
    let ws = crate::workspace::Workspace::from_session(&name, &session);
    match crate::workspace::save_ws(&path, &ws) {
        Ok(()) => {
            if let Some(s) = state_mut(hwnd) {
                s.ws_source = Some(ws.name);
            }
        }
        Err(e) => ws_report(hwnd, &format!("Could not save workspace: {e}")),
    }
}

fn overwrite_workspace(hwnd: HWND) {
    let Some(path) = crate::workspace::store_path() else {
        return;
    };
    let Some(s) = state_ref(hwnd) else {
        return;
    };
    let Some(name) = s.ws_source.clone() else {
        return;
    };
    let session = snapshot_session(s, hwnd);
    let ws = crate::workspace::Workspace::from_session(&name, &session);
    if let Err(e) = crate::workspace::save_ws(&path, &ws) {
        ws_report(hwnd, &format!("Could not overwrite {name:?}: {e}"));
    }
}

fn rename_workspace(hwnd: HWND) {
    let Some(path) = crate::workspace::store_path() else {
        return;
    };
    let Some(old) = state_ref(hwnd).and_then(|s| s.ws_source.clone()) else {
        return;
    };
    let Some(new) = prompt_workspace_name(hwnd, "Rename workspace", &old) else {
        return;
    };
    match crate::workspace::rename_ws(&path, &old, &new) {
        Ok(()) => {
            if let Some(s) = state_mut(hwnd) {
                s.ws_source = Some(new);
            }
        }
        Err(e) => ws_report(hwnd, &format!("Could not rename workspace: {e}")),
    }
}

fn delete_workspace_by_index(hwnd: HWND, idx: usize) {
    let Some(name) = crate::workspace::names().get(idx).cloned() else {
        return;
    };
    let Some(path) = crate::workspace::store_path() else {
        return;
    };
    let question = format!("Delete workspace {:?} from workspaces.toml?", name);
    if !ws_confirm(hwnd, &question, "Delete workspace") {
        return;
    }
    if let Err(e) = crate::workspace::delete_ws(&path, &name) {
        ws_report(hwnd, &format!("Could not delete workspace: {e}"));
    }
}

/// WORKSPACE-007: modal name prompt built from an in-memory dialog
/// template (no .rc resource): a label, an edit seeded with `initial`,
/// OK and Cancel. Returns the typed name on OK, `None` on Cancel.
fn prompt_workspace_name(parent: HWND, title: &str, initial: &str) -> Option<String> {
    let mut answer = initial.to_owned();
    let template = ws_name_dialog_template(title);
    let hinstance = unsafe { GetModuleHandleW(None) }.ok()?;
    let dlg: DLGPROC = Some(ws_name_dlgproc);
    let res = unsafe {
        DialogBoxIndirectParamW(
            Some(hinstance.into()),
            template.as_ptr().cast::<DLGTEMPLATE>(),
            Some(parent),
            dlg,
            LPARAM(&mut answer as *mut String as isize),
        )
    };
    (res == IDOK.0 as isize).then_some(answer)
}

/// Pack a UTF-16 word stream into the DWORD-aligned buffer a dialog
/// template pointer must be.
fn pack_words(words: Vec<u16>) -> Vec<u32> {
    words
        .chunks(2)
        .map(|c| u32::from(c[0]) | c.get(1).map_or(0, |w| u32::from(*w) << 16))
        .collect()
}

fn dlg_push_str(words: &mut Vec<u16>, s: &str) {
    words.extend(s.encode_utf16());
    words.push(0);
    if words.len() % 2 == 1 {
        words.push(0);
    }
}

fn dlg_push_atom(words: &mut Vec<u16>, atom: u16) {
    words.extend([0xFFFF, atom]);
}

fn dlg_item(words: &mut Vec<u16>, style: u32, id: u16, class: u16, rect: [i16; 4], text: &str) {
    let [x, y, cx, cy] = rect;
    words.extend([
        (style & 0xFFFF) as u16,
        (style >> 16) as u16,
        0,
        0,
        x as u16,
        y as u16,
        cx as u16,
        cy as u16,
        id,
    ]);
    dlg_push_atom(words, class);
    dlg_push_str(words, text);
}

/// The name prompt layout: label, edit, OK, Cancel (dialog units).
/// Built to the documented DLGTEMPLATE layout: dialog header, then four
/// DLGITEMTEMPLATE records (style, extended style, x/y/cx/cy, id, class
/// atom, text), every field WORD-aligned.
fn ws_name_dialog_template(title: &str) -> Vec<u32> {
    const WS_STATIC: u32 = WS_CHILD.0 | WS_VISIBLE.0; // SS_LEFT is 0
    const WS_EDIT: u32 = WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | WS_BORDER.0;
    const WS_BTN: u32 = WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0;
    const ID_EDIT: u16 = 1000;
    const ID_OK: u16 = 1; // IDOK
    const ID_CANCEL: u16 = 2; // IDCANCEL
    let style = WS_POPUP.0 | DS_MODALFRAME as u32 | WS_CAPTION.0 | WS_SYSMENU.0;
    let mut w: Vec<u16> = vec![
        (style & 0xFFFF) as u16,
        (style >> 16) as u16,
        0,
        0,
        4, // item count
        0,
        0,
        220,
        42, // dialog size in dialog units
        0,
        0, // menu: none
        0,
        0, // window class: default
    ];
    dlg_push_str(&mut w, title);
    dlg_item(
        &mut w,
        WS_STATIC,
        1001,
        0x0082,
        [10, 8, 200, 10],
        "Workspace name:",
    );
    dlg_item(&mut w, WS_EDIT, ID_EDIT, 0x0081, [10, 20, 200, 12], "");
    dlg_item(
        &mut w,
        WS_BTN | BS_DEFPUSHBUTTON as u32,
        ID_OK,
        0x0080,
        [90, 35, 50, 14],
        "OK",
    );
    dlg_item(
        &mut w,
        WS_BTN | BS_PUSHBUTTON as u32,
        ID_CANCEL,
        0x0080,
        [145, 35, 50, 14],
        "Cancel",
    );
    pack_words(w)
}

unsafe extern "system" fn ws_name_dlgproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    // The edit is the second template control (after the label), which is
    // its child-window order.
    let edit = || -> Option<HWND> {
        let first = unsafe { GetWindow(hwnd, GW_CHILD) }.ok()?;
        unsafe { GetWindow(first, GW_HWNDNEXT) }.ok()
    };
    match msg {
        WM_INITDIALOG => {
            let seed = unsafe { &*(lparam.0 as *const String) };
            if let Some(edit) = edit() {
                let text = wstr(seed);
                let _ = unsafe { SetWindowTextW(edit, PCWSTR(text.as_ptr())) };
                let _ = unsafe { SetFocus(Some(edit)) };
            }
            1
        }
        WM_COMMAND => {
            if wparam.0 as u32 == IDOK.0 as u32 {
                let answer = unsafe { &mut *(lparam.0 as *mut String) };
                if let Some(edit) = edit() {
                    let mut buf = [0u16; 512];
                    let n = unsafe { GetWindowTextW(edit, &mut buf) };
                    *answer = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
                }
                let _ = unsafe { EndDialog(hwnd, IDOK.0 as isize) };
                1
            } else if wparam.0 as u32 == IDCANCEL.0 as u32 {
                let _ = unsafe { EndDialog(hwnd, IDCANCEL.0 as isize) };
                1
            } else {
                0
            }
        }
        _ => 0,
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
    /// TAB-026: session loaded at startup; consumed once the window exists
    /// (tabs rebuilt, shells respawned), `None` afterwards.
    restore: Option<crate::session::Session>,
    /// BELL-003/004: tabs whose bell is still flashing.
    bell_tabs: Vec<bool>,
    bell_flash: bool,
    /// Full configuration: tab bar placement, split, search, cursor,
    /// keybindings, profiles (CONFIG-007..013).
    cfg: Config,
    /// Built-in shells merged with the config `[[profile]]` entries.
    profiles: Vec<ProfileSpec>,
    /// Tab whose title is being renamed inline (TAB-013), the buffer being
    /// typed, `None` when not renaming.
    renaming: Option<(usize, String)>,
    /// Tab index being dragged to reorder (TAB-016).
    tab_drag: Option<usize>,
    /// Cursor blink phase: the timer flips it every `BLINK_MS`.
    blink_on: bool,
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
    /// OSC 8 link URI under the pointer when the context menu was opened;
    /// the Open/Copy Link menu items consume it (issue #41).
    link_target: Option<String>,
    /// Set when an app key binding consumed a `WM_KEYDOWN`; the matching
    /// `WM_CHAR` Windows posts afterwards must not reach the PTY.
    suppress_char: bool,
    /// Saved window rect + styles while fullscreen (F11), `None` in
    /// windowed mode.
    fullscreen: Option<(RECT, isize, isize)>,
    /// Issue #25: high half of a UTF-16 surrogate pair from `WM_CHAR`
    /// (VK_PACKET/IME deliver non-BMP characters as two messages).
    pending_surrogate: Option<u16>,
    /// Search overlay state (SEARCH-001..015); `None` when the overlay
    /// is closed. While active, keystrokes edit the query instead of
    /// reaching the shell.
    search: Option<SearchState>,
    /// PALETTE-001 (issue #39): open command palette, `None` while closed.
    palette: Option<crate::palette::PaletteState>,
    /// PALETTE-005: recently run palette labels, newest first, capped.
    recent: Vec<String>,
    /// Index of the selected built-in profile (MENU-008). New tabs and
    /// split panes spawn it.
    profile: usize,
    /// `themes/` beside the config, for per-profile theme files (THEME-009/011).
    theme_dir: Option<std::path::PathBuf>,
    /// Last seen config file mtime; a change triggers a CONFIG-018 reload.
    /// `None` until the first poll tick records it, so a reload never fires on
    /// startup.
    cfg_mtime: Option<std::time::SystemTime>,
    /// Roadmap v0.2: this is the quick (drop-down) window. It hides instead
    /// of closing, skips the session file, and never quits the app.
    quick: bool,
    /// WORKSPACE-003: name of the workspace template this window was
    /// opened from (native UI or `--workspace`). "Overwrite workspace"
    /// writes the current layout back to it; `None` when the window was
    /// not opened from a template.
    ws_source: Option<String>,
    /// WORKSPACE-006: per-tab leaf commands of the template being opened,
    /// flatten order, consumed by `spawn_restored`.
    ws_pending: Option<Vec<Vec<Option<Vec<String>>>>>,
    /// SHELLINT-025: id of the last command whose completion this window
    /// already acknowledged (notified, or foreground at the time), so each
    /// command earns at most one toast.
    cmd_notified: u64,
    /// A sizing drag is in progress (`WM_ENTERSIZEMOVE`..`WM_EXITSIZEMOVE`):
    /// the render target follows live, the grid/ConPTY reflow lands once at
    /// the end (mid-drag reflow was the visible text bounce).
    drag_resize: bool,
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

    /// Y of the tab-bar strip (TAB-022: top or bottom of the window).
    fn bar_top(&self, height: f32) -> f32 {
        if self.cfg.tab_bar.placement == TabBarPlacement::Bottom {
            (height - self.bar_height()).max(0.0)
        } else {
            0.0
        }
    }

    /// Y where the grid starts, i.e. below (or above) the tab bar, plus the
    /// window padding (WINDOW-011).
    fn grid_top(&self) -> f32 {
        let bar = if self.cfg.tab_bar.placement == TabBarPlacement::Bottom {
            0.0
        } else {
            self.bar_height()
        };
        bar + self.pad_y()
    }

    /// `[window.padding]` in DIPs, matched to what the renderer insets by.
    fn pad_x(&self) -> f32 {
        self.cfg.window.padding.x as f32
    }

    fn pad_y(&self) -> f32 {
        self.cfg.window.padding.y as f32
    }

    /// `true` when `y` (client DIPs) lands on the tab-bar strip.
    /// CONFIG-018: reload when the config file changes on disk.
    ///
    /// shortcut: one `fs::metadata` per poll tick, which is cheap; move to
    /// `ReadDirectoryChangesW` if a trace ever shows it costing anything. No
    /// unit test: the state machine lives on `WindowState`, which needs an HWND.
    fn watch_config(&mut self, hwnd: HWND) {
        let Some(path) = crate::config::config_path() else {
            return;
        };
        let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            return;
        };
        if self.cfg_mtime == Some(mtime) {
            return;
        }
        let first = self.cfg_mtime.is_none();
        self.cfg_mtime = Some(mtime);
        if !first {
            reload_config(self, hwnd);
        }
    }

    /// BELL-001..008: ring the bells the shells just produced. A bell that
    /// arrives while a flash is running only re-arms the timer.
    fn take_bell(&mut self, hwnd: HWND) {
        if !self.app.any_bell() {
            return;
        }
        self.bell_tabs = self.app.bell_flags();
        self.app.take_bells();
        if self.cfg.bell.audible() {
            // BELL-009: MessageBeep returns straight away, so a storm of BELs
            // cannot stall the PTY read.
            unsafe {
                let _ = MessageBeep(MB_ICONASTERISK);
            }
        }
        if self.cfg.bell.visual() {
            self.bell_flash = true;
            unsafe {
                let _ = SetTimer(Some(hwnd), BELL_TIMER, BELL_MS, None);
            }
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
        }
        // BELL-005: when the window is in the background, say so out loud.
        let backgrounded = unsafe { GetForegroundWindow() } != hwnd;
        if backgrounded {
            let ringing = self
                .bell_tabs
                .iter()
                .position(|flag| *flag)
                .and_then(|i| self.app.tabs().get(i))
                .map_or_else(String::new, |tab| tab.title().to_string());
            show_toast(
                &ringing,
                if self.app.any_bell() {
                    "A command is waiting for you."
                } else {
                    "A command finished."
                },
            );
        }
    }

    /// SHELLINT-024 (issue #38): a finished command at least
    /// `command-notify-ms` long earns one completion toast while the
    /// window is in the background; 0 disables the toasts.
    fn notify_finished_command(&mut self, hwnd: HWND) {
        let Some(tab) = self.app.active_tab() else {
            return;
        };
        let pane = tab.pane();
        let Some(last) = pane.commands().last_finished() else {
            return;
        };
        let id = last.id;
        if self.cmd_notified == id {
            return;
        }
        // Acknowledge first: a foreground visit, and a command that can
        // never notify (no exit code, disabled threshold), both retire the
        // id so switching away later never toasts about an old command.
        self.cmd_notified = id;
        let Some(exit) = last.exit else { return };
        let threshold = self.cfg.command_notify_ms;
        if threshold == 0 || !last.duration_ms().is_some_and(|ms| ms >= threshold) {
            return;
        }
        if unsafe { GetForegroundWindow() } == hwnd {
            return;
        }
        let cmd = pane
            .grid()
            .line_text_abs(last.start_abs)
            .unwrap_or_default();
        show_toast(
            tab.title(),
            &format!("Command finished (exit {exit})\n{cmd}"),
        );
    }

    fn over_tab_bar(&self, y: f32, height: f32) -> bool {
        let bar = self.bar_height();
        bar > 0.0 && y >= self.bar_top(height) && y < self.bar_top(height) + bar
    }

    /// `true` when `(x, y)` (client DIPs) lands on tab `idx`, on the `+`
    /// button, or on the close box of tab `idx` (CONFIG-010).
    fn tab_hit(&self, x: f32, y: f32, height: f32) -> TabHit {
        let bar = self.bar_height();
        let top = self.bar_top(height);
        if bar == 0.0 || y < top || y >= top + bar {
            return TabHit::None;
        }
        let layout = match self.renderer.as_ref() {
            Some(r) => r.tab_layout(),
            None => return TabHit::None,
        };
        if x >= layout.plus.0 && x < layout.plus.1 {
            return TabHit::Plus;
        }
        for (idx, &(lo, hi)) in layout.tabs.iter().enumerate() {
            if x >= lo && x < hi {
                let close_w = self.metrics.cell_width * 1.5;
                if self.cfg.tab_bar.close_button && x >= hi - close_w {
                    return TabHit::Close(idx);
                }
                return TabHit::Tab(idx);
            }
        }
        TabHit::None
    }
}

/// What a tab-bar click landed on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TabHit {
    Tab(usize),
    Close(usize),
    Plus,
    None,
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
            // Issue #40: accept Explorer file drops.
            unsafe { DragAcceptFiles(hwnd, true) };
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
        WM_ENTERSIZEMOVE => {
            if let Some(s) = state_mut(hwnd) {
                s.drag_resize = true;
            }
            LRESULT(0)
        }
        WM_EXITSIZEMOVE => {
            // The drag's final geometry lands here: one grid/ConPTY reflow.
            if let Some(s) = state_mut(hwnd) {
                s.drag_resize = false;
                if let Some(r) = &mut s.renderer {
                    let (cols, rows) = r.grid_size().max((MIN_COLS, MIN_ROWS));
                    if s.app.resize_active(cols, rows).is_err() {
                        eprintln!("rustty: ConPTY resize failed");
                    }
                }
            }
            LRESULT(0)
        }
        WM_SIZE => {
            // Minimizing sends a zero client rect; resizing the grid to a
            // 1x1 render target would truncate retained lines (issue #16),
            // so the last usable geometry stays in place while minimized.
            if wparam.0 as u32 != SIZE_MINIMIZED {
                let width = ((lparam.0 & 0xFFFF) as u16) as u32;
                let height = (((lparam.0 >> 16) & 0xFFFF) as u16) as u32;
                resize_window(hwnd, width, height);
            }
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
                let prompt = s
                    .renaming
                    .as_ref()
                    .map(|(_, buf)| format!("Rename tab: {buf}  (Enter to accept, Esc to cancel)"));
                // NFR-216: paint() logs and swallows Direct2D failures.
                let active = s.app.active_index();
                r.set_bell_state(
                    s.bell_flash,
                    &s.bell_tabs,
                    s.bell_tabs.get(active).copied().unwrap_or(false),
                );
                r.paint(
                    &s.app,
                    &s.theme,
                    s.search.as_ref(),
                    s.palette.as_ref(),
                    prompt.as_deref(),
                );
            }
            let _ = unsafe { EndPaint(hwnd, &ps) };
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1), // Direct2D owns the whole client area
        WM_APP => {
            // Issue #40: discovery finished on its worker thread; re-merge
            // the profile list, rebuild the menu and Jump List, refresh the
            // palette registry. An empty result leaves everything as-is.
            let found = discovered_now();
            if !found.is_empty()
                && let Some(s) = state_mut(hwnd)
            {
                s.profiles = merge_profiles(&s.cfg, &found);
                s.profile = s.profile.min(s.profiles.len() - 1);
                s.palette = None;
                if !s.quick
                    && let Ok(menu) =
                        build_menu(&s.profiles, &menu_theme_names(s.theme_dir.as_deref()))
                {
                    let _ = unsafe { SetMenu(hwnd, Some(menu)) };
                }
                let names: Vec<String> = s.profiles.iter().map(|p| p.name.clone()).collect();
                rebuild_jumplist(&names);
            }
            LRESULT(0)
        }
        WM_DROPFILES => {
            handle_drop(hwnd, HDROP(wparam.0 as *mut core::ffi::c_void));
            LRESULT(0)
        }
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
        WM_IME_STARTCOMPOSITION => {
            position_ime_window(hwnd);
            LRESULT(0)
        }
        WM_IME_COMPOSITION => {
            handle_ime_composition(hwnd, lparam);
            LRESULT(0)
        }
        WM_IME_CHAR => {
            // Fallback path for IMEs that deliver committed characters
            // directly instead of through GCS_RESULTSTR.
            if take_suppressed_char(hwnd) {
                return LRESULT(0);
            }
            handle_char(hwnd, wparam.0 as u32, false);
            LRESULT(0)
        }
        WM_IME_ENDCOMPOSITION => LRESULT(0),
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
            // Only claim client-area hits (I-beam/arrow over the tab bar).
            // Non-client hit codes (HTLEFT..HTBOTTOMRIGHT sizing frame,
            // HTCAPTION, HTMINBUTTON...) must fall through to
            // DefWindowProcW so it sets the sizing/move cursors; claiming
            // every hit made border drag-resize cursorless.
            let hit = (lparam.0 & 0xFFFF) as u32;
            if hit == HTCLIENT {
                set_cursor_for_position(wparam);
                LRESULT(1)
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
        }
        WM_TIMER => {
            if wparam.0 == BLINK_TIMER {
                blink_cursor(hwnd);
            } else if wparam.0 == BELL_TIMER {
                // BELL-003/004: one flash, for a fixed time.
                if let Some(state) = state_mut(hwnd) {
                    state.bell_flash = false;
                    state.bell_tabs.clear();
                }
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            } else if wparam.0 == MARQUEE_TIMER {
                let running = state_mut(hwnd)
                    .and_then(|s| s.renderer.as_mut())
                    .is_some_and(|r| r.advance_marquee());
                if running {
                    invalidate_tab_bar(hwnd);
                } else {
                    let _ = unsafe { KillTimer(Some(hwnd), MARQUEE_TIMER) };
                }
            } else if wparam.0 == SEARCH_TIMER {
                search_step_tick(hwnd);
            }
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
        WM_KILLFOCUS => {
            // Roadmap v0.2: the quick terminal vanishes when you click away.
            if state_ref(hwnd).is_some_and(|s| s.quick && s.cfg.quick_terminal.autohide) {
                let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            }
            LRESULT(0)
        }
        WM_HOTKEY => {
            if wparam.0 as i32 == QUICK_HOTKEY_ID {
                toggle_quick();
            }
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
            if state_ref(hwnd).is_some_and(|s| s.quick) {
                // Roadmap v0.2: the quick terminal hides, it never closes,
                // so its shell survives the X, Alt+F4 and the hotkey.
                let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
                return LRESULT(0);
            }
            // WINDOW-014: quitting asks first when the profile wants it.
            let ask = state_ref(hwnd).is_some_and(|s| s.app.tab_confirm_close());
            if ask && !confirm_yes_no(hwnd, "Quit rustty?") {
                return LRESULT(0);
            }
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(ptr) = reclaim_state(hwnd) {
                // SAFETY: WM_CREATE stored this pointer; the window is
                // going away, so take ownership back and drop the app
                // (which closes every shell).
                let mut state = unsafe { Box::from_raw(ptr) };
                if state.quick {
                    // Roadmap v0.2: the quick window is leaving (the main
                    // window is quitting, or its shell died). It owns no
                    // session slot, so no save.
                    QUICK_HWND.store(0, Ordering::Release);
                } else {
                    // TAB-026/WINDOW-015/016: save while every shell is still
                    // live (fresh OSC 7 cwds); a failed save never blocks quit.
                    if state.cfg.session_restore {
                        let session = snapshot_session(&state, hwnd);
                        if let Err(e) = crate::session::save(&session) {
                            eprintln!("rustty: session save failed: {e}");
                        }
                    }
                    // The quick window rides along with the app.
                    let quick = HWND(QUICK_HWND.load(Ordering::Acquire) as *mut c_void);
                    if !quick.is_invalid() && unsafe { IsWindow(Some(quick)).as_bool() } {
                        let _ = unsafe { DestroyWindow(quick) };
                    }
                }
                state.app.close_all_shells();
            }
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // WIN-007 / UIA-001: hand UI Automation this window's root
        // provider. Other object ids (the native menu's MSAA bridge) go
        // to DefWindowProc.
        WM_GETOBJECT if lparam.0 as i32 == UiaRootObjectId => {
            match ComObject::from(Provider::new(hwnd)).cast::<IRawElementProviderSimple>() {
                Ok(provider) => unsafe {
                    UiaReturnRawElementProvider(hwnd, wparam, lparam, &provider)
                },
                Err(_) => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
            }
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
    let quick = state_ref(hwnd).is_some_and(|s| s.quick);
    // Native menu bar (MENU-001..005). A menu failure leaves the terminal
    // usable, matching the no-fatal-UI-failure rule. The frameless quick
    // window has no menu, and it is not shown until the hotkey summons it.
    if !quick {
        let menu = state_ref(hwnd)
            .map(|s| build_menu(&s.profiles, &menu_theme_names(s.theme_dir.as_deref())));
        if let Some(menu) = menu.and_then(|r| r.ok()) {
            unsafe {
                let _ = SetMenu(hwnd, Some(menu));
            }
        }
        let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
    }
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
            eprintln!("rustty: font measurement failed: {e}");
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
            eprintln!("rustty: Direct2D renderer unavailable: {e}");
            None
        }
    };
    let mut restored = false;
    if let Some(s) = state_mut(hwnd) {
        s.renderer = renderer;
        sync_chrome(s);
        if s.restore.is_some() {
            // TAB-026: rebuild saved tabs; shells spawn after the tab bar
            // has taken its final height.
            restore_tabs(s);
            restored = true;
        } else {
            // The first tab belongs to the starting profile (CONFIG-013). Its
            // theme and font (SHELL-019) apply before the grid is measured: a
            // different font means a different grid.
            let spec = active_profile(s);
            apply_profile_theme(s, &spec);
            apply_profile_font(s, &spec);
            let (cols, rows) = s
                .renderer
                .as_ref()
                .map_or((init_cols, init_rows), |r| r.grid_size());
            let cols = cols.max(MIN_COLS);
            let rows = rows.max(MIN_ROWS);
            if let Some(tab) = s.app.active_tab_mut() {
                tab.set_profile(spec.name.clone());
                tab.set_auto_title(spec.name.clone());
                tab.set_color(spec.color);
            }
            // Spawn first, then resize: a resize must reach a live ConPTY, and
            // the shell must start at the window's real grid size (NFR-216:
            // these failures are logged, never fatal).
            if let Err(e) =
                s.app
                    .spawn_active(&spec_opts(&spec, cols, rows, s.cfg.shell_integration))
            {
                eprintln!("rustty: shell spawn failed: {e}");
            }
            if let Err(e) = s.app.resize_active(cols, rows) {
                eprintln!("rustty: initial resize failed: {e}");
            }
        }
    }
    if restored {
        // The bar height follows the restored tab count (TAB-010 "auto");
        // grid and every tab must follow before shells spawn.
        sync_tab_bar(hwnd);
        if let Some(s) = state_mut(hwnd) {
            spawn_restored(s);
        }
    }
    if let Some(s) = state_mut(hwnd) {
        s.timer_id = unsafe { SetTimer(Some(hwnd), POLL_TIMER, POLL_MS, Some(poll_timer)) };
        if s.cfg.cursor_blink {
            // CONFIG-009: blink the focused cursor.
            s.blink_on = true;
            unsafe { SetTimer(Some(hwnd), BLINK_TIMER, BLINK_MS, None) };
        }
        set_title(hwnd, &s.app);
        let _ = unsafe { SetFocus(Some(hwnd)) };
    }
    apply_opacity(hwnd);
}

/// CONFIG-011: window opacity through a layered window. A config value of
/// 100 (the default) leaves the window non-layered, so nothing changes.
fn apply_opacity(hwnd: HWND) {
    let Some(opacity) = state_ref(hwnd).map(|s| s.cfg.opacity) else {
        return;
    };
    if opacity >= 100 {
        return;
    }
    let alpha = (u32::from(opacity) * 255 / 100) as u8;
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) | WS_EX_LAYERED.0 as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
    }
}

// ---------------------------------------------------------------------------
// Quick terminal (roadmap v0.2 "Hotkey summon from screen edge")
// ---------------------------------------------------------------------------

/// Show the quick terminal docked to its screen edge, hide it when it is
/// already up, and (re)create it when its shell ended and the window died.
fn toggle_quick() {
    let Some(cfg) = QUICK_SPEC
        .lock()
        .ok()
        .as_ref()
        .and_then(|slot| slot.as_ref().map(|(cfg, ..)| cfg.clone()))
    else {
        return;
    };
    let handle = QUICK_HWND.load(Ordering::Acquire);
    if handle == 0 {
        create_quick_window();
    }
    let handle = QUICK_HWND.load(Ordering::Acquire);
    if handle == 0 {
        return;
    }
    let quick = HWND(handle as *mut c_void);
    unsafe {
        if IsWindowVisible(quick).as_bool() {
            let _ = ShowWindow(quick, SW_HIDE);
        } else {
            dock_quick(quick, &cfg);
            let _ = ShowWindow(quick, SW_SHOW);
            let _ = SetForegroundWindow(quick);
        }
    }
}

/// Size and place the quick window: the monitor's full work-area width,
/// `quick-terminal.height` rows tall, flush with the configured edge.
fn dock_quick(quick: HWND, cfg: &Config) {
    let metrics = match state_ref(quick) {
        Some(s) => s.metrics,
        None => return,
    };
    let monitor = unsafe { MonitorFromWindow(quick, MONITOR_DEFAULTTOPRIMARY) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return;
    }
    let work = info.rcWork;
    // The window rect gets no frame (WS_POPUP), so it is pure grid.
    let (_, height) = px_size(metrics, 1, cfg.quick_terminal.height, 0.0);
    let y = match cfg.quick_terminal.edge {
        QuickEdge::Top => work.top,
        QuickEdge::Bottom => work.bottom - height,
    };
    unsafe {
        let _ = SetWindowPos(
            quick,
            Some(HWND_TOPMOST),
            work.left,
            y,
            work.right - work.left,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Build the hidden quick window from [`QUICK_SPEC`]. No menu, no frame
/// (`WS_POPUP`), not in the taskbar (`WS_EX_TOOLWINDOW`), above everything
/// (`WS_EX_TOPMOST`); shown by [`toggle_quick`]. A creation failure is
/// logged, not fatal (NFR-216).
fn create_quick_window() {
    let Some((cfg, theme, spawn)) = QUICK_SPEC.lock().ok().and_then(|slot| slot.clone()) else {
        return;
    };
    let dpi = unsafe {
        let d = GetDpiForSystem();
        if d == 0 { 96 } else { d }
    };
    let font = cfg.font();
    let metrics = match measure_cells(dpi, &font) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("rustty: quick terminal font measurement failed: {e}");
            return;
        }
    };
    let state = Box::new(WindowState {
        restore: None,
        ws_source: None,
        ws_pending: None,
        cmd_notified: 0,
        drag_resize: false,
        app: {
            let mut app = App::with_size(spawn.cols, spawn.rows, cfg.scrollback);
            app.set_show_cwd(cfg.tab_bar.show_cwd);
            app
        },
        theme,
        spawn,
        font,
        tab_bar: cfg.tab_bar.visible,
        bell_tabs: Vec::new(),
        bell_flash: false,
        cfg_mtime: None,
        quick: true,
        profiles: merge_profiles(&cfg, &discovered_now()),
        cfg,
        renaming: None,
        tab_drag: None,
        blink_on: true,
        theme_dir: theme_dir_from_config(),
        renderer: None,
        metrics,
        timer_id: 0,
        drag_anchor: None,
        last_click: None,
        link_target: None,
        suppress_char: false,
        pending_surrogate: None,
        fullscreen: None,
        search: None,
        palette: None,
        recent: Vec::new(),
        profile: 0,
    });
    let state_param = Box::into_raw(state) as *const c_void;
    let hinstance = match unsafe { GetModuleHandleW(None) } {
        Ok(h) => h,
        Err(e) => {
            eprintln!("rustty: quick terminal skipped: {e}");
            unsafe { drop(Box::from_raw(state_param as *mut WindowState)) };
            return;
        }
    };
    let class_name = wstr(CLASS_NAME);
    let title = wstr(APP_NAME);
    let created = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_POPUP,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None,
            Some(hinstance.into()),
            Some(state_param),
        )
    };
    match created {
        Ok(hwnd) => QUICK_HWND.store(hwnd.0 as isize, Ordering::Release),
        Err(e) => {
            // The window never existed to reclaim the box; free it here.
            unsafe { drop(Box::from_raw(state_param as *mut WindowState)) };
            eprintln!("rustty: quick terminal window failed: {e}");
        }
    }
}

/// Register the system-wide summon hotkey. The spec was validated at config
/// load; a key another app grabbed (or a VK this layout cannot name) is
/// reported on stderr and the quick terminal stays off.
fn register_hotkey(main: HWND, cfg: &QuickTerminalConfig) -> std::result::Result<(), ()> {
    let Ok(spec) = config::parse_hotkey(&cfg.key) else {
        return Err(());
    };
    let name = spec.rsplit('+').next().unwrap_or("");
    let vk = match hotkey_vk(name) {
        Some(vk) => vk,
        None => return Err(()),
    };
    let mut mods = MOD_NOREPEAT;
    for m in spec.split('+').take_while(|p| *p != name) {
        mods |= match m {
            "ctrl" => MOD_CONTROL,
            "alt" => MOD_ALT,
            "shift" => MOD_SHIFT,
            _ => MOD_NOREPEAT,
        };
    }
    let _ = unsafe { UnregisterHotKey(Some(main), QUICK_HOTKEY_ID) };
    unsafe { RegisterHotKey(Some(main), QUICK_HOTKEY_ID, mods, u32::from(vk)) }.map_err(|_| ())
}

/// Virtual-key code for a hotkey key name (the mirror of [`key_name`],
/// plus the punctuation the keybinding specs accept). `plus` shares the
/// `=` key, the same binding Windows itself shows.
fn hotkey_vk(name: &str) -> Option<u16> {
    let vk = match name {
        "backspace" => 0x08,
        "tab" => 0x09,
        "enter" => 0x0D,
        "escape" => 0x1B,
        "space" => 0x20,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" => 0x2D,
        "delete" => 0x2E,
        "minus" => 0x6D,
        "equal" | "plus" => 0xBB,
        "grave" => 0xC0,
        "comma" => 0xBC,
        "period" => 0xBE,
        "slash" => 0xBF,
        "left-bracket" => 0xDB,
        "right-bracket" => 0xDD,
        "backslash" => 0xDC,
        "semicolon" => 0xBA,
        "quote" => 0xDE,
        _ => 0,
    };
    if vk != 0 {
        return Some(vk);
    }
    if name.len() == 1 {
        let c = name.as_bytes()[0];
        if c.is_ascii_alphabetic() {
            return Some(u16::from(c.to_ascii_uppercase()));
        }
        if c.is_ascii_digit() {
            return Some(u16::from(c));
        }
    }
    if name.len() > 1 && name.starts_with('f') {
        let n = name[1..].parse::<u16>().ok()?;
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    None
}

/// The profile a saved tab names, falling back to the starting profile
/// when the config no longer has it (TAB-026).
fn profile_spec_named(s: &WindowState, name: &str) -> ProfileSpec {
    s.profiles
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .cloned()
        .unwrap_or_else(|| active_profile(s))
}

/// TAB-026: rebuild the saved tabs (panes, titles, focus, profile chrome).
/// Shells spawn later in [`spawn_restored`], once the tab bar has taken
/// its final height. The placeholder tab created at startup is dropped.
fn restore_tabs(s: &mut WindowState) {
    let Some(session) = s.restore.as_ref() else {
        return;
    };
    let _ = s.app.close_tab(0);
    for st in &session.tabs {
        let spec = profile_spec_named(s, &st.profile);
        let scrollback = spec.scrollback.unwrap_or(s.cfg.scrollback);
        let (cols, rows) = s
            .renderer
            .as_ref()
            .map_or((s.spawn.cols, s.spawn.rows), |r| r.grid_size());
        let idx = s.app.add_tab_shape(
            &st.shape,
            cols.max(MIN_COLS),
            rows.max(MIN_ROWS),
            scrollback,
            spec.name.clone(),
            spec.color,
        );
        if let Some(tab) = s.app.tab_at_mut(idx) {
            tab.set_confirm(spec.confirm_close);
            // A user rename wins over the shell's title (TAB-013).
            if !st.title.is_empty() && !tab.title_is_fixed() {
                if st.title_fixed {
                    tab.set_title(st.title.clone());
                } else {
                    tab.set_auto_title(st.title.clone());
                }
            }
            let _ = tab.focus_index(st.focused_pane);
        }
    }
    // One renderer serves the window, so theme and font follow the
    // focused tab's profile (SHELL-019).
    let focused = session.focused_tab.min(s.app.tab_count().saturating_sub(1));
    s.app.select_tab(focused);
    let name = s
        .app
        .active_tab()
        .map(|t| t.profile().to_owned())
        .unwrap_or_default();
    let spec = profile_spec_named(s, &name);
    s.profile = s
        .profiles
        .iter()
        .position(|p| p.name == spec.name)
        .unwrap_or(0);
    apply_profile_theme(s, &spec);
    apply_profile_font(s, &spec);
}

/// WORKSPACE-006: per-tab leaf commands of a template, flatten order,
/// the same order as the cwds `spawn_restored` collects.
fn ws_leaf_cmds(ws: &crate::workspace::Workspace) -> Vec<Vec<Option<Vec<String>>>> {
    ws.tab
        .iter()
        .map(|t| {
            let mut cmds = Vec::new();
            t.shape.leaf_commands(&mut cmds);
            cmds
        })
        .collect()
}

/// TAB-026: resize every restored tab to the current grid and respawn
/// each pane's shell in its saved directory. Consumes the session.
/// WORKSPACE-006: a workspace template's per-leaf commands override the
/// profile's program where set.
fn spawn_restored(s: &mut WindowState) {
    let Some(session) = s.restore.take() else {
        return;
    };
    let ws_cmds = s.ws_pending.take();
    let (cols, rows) = s
        .renderer
        .as_ref()
        .map_or((s.spawn.cols, s.spawn.rows), |r| r.grid_size());
    let (cols, rows) = (cols.max(MIN_COLS), rows.max(MIN_ROWS));
    if let Err(e) = s.app.resize_all_tabs(cols, rows) {
        eprintln!("rustty: restored tab resize failed: {e}");
    }
    for (i, st) in session.tabs.iter().enumerate() {
        let spec = profile_spec_named(s, &st.profile);
        let mut cwds = Vec::new();
        st.shape.leaf_cwds(&mut cwds);
        let opts = spec_opts(&spec, cols, rows, s.cfg.shell_integration);
        let cmds: &[Option<Vec<String>>] = ws_cmds
            .as_deref()
            .and_then(|tabs| tabs.get(i))
            .map_or(&[], Vec::as_slice);
        if let Some(tab) = s.app.tab_at_mut(i)
            && let Err(e) = tab.spawn_panes_cmds(&opts, &cwds, cmds)
        {
            eprintln!("rustty: shell spawn failed: {e}");
        }
    }
}

/// TAB-026/WINDOW-015/016: snapshot the run for `session.toml`. The
/// window rect is skipped in fullscreen (the fullscreen rect is not a
/// windowed geometry to come back to).
fn snapshot_session(s: &WindowState, hwnd: HWND) -> crate::session::Session {
    let tabs = s
        .app
        .tabs()
        .iter()
        .map(|t| crate::session::SessionTab {
            profile: t.profile().to_owned(),
            title: t.title().to_owned(),
            title_fixed: t.title_is_fixed(),
            focused_pane: t.focused_index().unwrap_or(0),
            shape: t.shape(),
        })
        .collect();
    let mut rc = RECT::default();
    let window = if s.fullscreen.is_some() {
        None
    } else if unsafe { GetWindowRect(hwnd, &mut rc) }.is_ok()
        && rc.right > rc.left
        && rc.bottom > rc.top
    {
        Some(crate::session::WindowRect {
            x: rc.left,
            y: rc.top,
            w: rc.right - rc.left,
            h: rc.bottom - rc.top,
        })
    } else {
        None
    };
    crate::session::Session {
        tabs,
        focused_tab: s.app.active_tab_index(),
        window,
    }
}

/// CONFIG-009: flip the cursor blink phase and repaint.
fn blink_cursor(hwnd: HWND) {
    let Some(s) = state_mut(hwnd) else { return };
    if !s.cfg.cursor_blink {
        return;
    }
    s.blink_on = !s.blink_on;
    if let Some(r) = &mut s.renderer {
        r.set_cursor_blink_visible(s.blink_on);
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// WM_SIZE follow-up: resize render target -> grid -> ConPTY (NFR resize
/// chain, spec NFR).
fn resize_window(hwnd: HWND, width_px: u32, height_px: u32) {
    let Some(s) = state_mut(hwnd) else { return };
    let Some(r) = &mut s.renderer else { return };
    r.resize(width_px, height_px);
    // Mid-drag the render target follows live and the grid/ConPTY reflow is
    // deferred to WM_EXITSIZEMOVE; repeated mid-drag reflow was the bounce.
    if !s.drag_resize {
        let (cols, rows) = r.grid_size().max((MIN_COLS, MIN_ROWS));
        if s.app.resize_active(cols, rows).is_err() {
            eprintln!("rustty: ConPTY resize failed");
        }
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Issue #9: an inactive tab keeps the geometry from when it was last
/// shown; the window may have been resized since. Give the newly active
/// tab the window's current client area.
fn sync_active_tab_geometry(hwnd: HWND) {
    let mut rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) }.is_err() {
        return;
    }
    let width = (rect.right - rect.left).max(0) as u32;
    let height = (rect.bottom - rect.top).max(0) as u32;
    resize_window(hwnd, width, height);
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
        eprintln!("rustty: ConPTY resize failed");
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// WM_KEYDOWN/WM_SYSKEYDOWN: app bindings first, then VT encoding.
fn handle_keydown(hwnd: HWND, vk: u16) {
    let mods = read_mods();
    // `[keybindings]` first: a user binding overrides the built-in shortcut
    // (KEY-022..025, CONFIG-008).
    let action = state_ref(hwnd).and_then(|s| s.cfg.keymap().get(&key_spec(vk, mods)).copied());
    if let Some(action) = action {
        run_key_action(hwnd, action);
        if let Some(s) = state_mut(hwnd) {
            s.suppress_char = true;
        }
        return;
    }
    // Tab rename mode: the keys edit the title, not the shell (TAB-013).
    if state_ref(hwnd).is_some_and(|s| s.renaming.is_some()) && handle_rename_keydown(hwnd, vk) {
        return;
    }
    // PALETTE-001 (issue #39): Ctrl+Shift+P opens or closes the palette,
    // checked before the overlay's key swallow so the chord toggles. A user
    // binding for the same chord won above (the keymap is checked first).
    if mods.ctrl && mods.shift && !mods.alt && vk == 0x50 {
        toggle_palette(hwnd);
        return;
    }
    // While the palette is open, every keydown is swallowed: the handled
    // keys drive the palette, and printables reach WM_CHAR, which edits
    // the query instead of the PTY. Nothing reaches the shell.
    if state_ref(hwnd).is_some_and(|s| s.palette.is_some()) {
        handle_palette_keydown(hwnd, vk);
        return;
    }
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
            0x26 => {
                // Ctrl+Shift+Up: jump to the previous command (SHELLINT-022)
                command_jump(hwnd, -1);
                true
            }
            0x28 => {
                // Ctrl+Shift+Down: jump to the next command (SHELLINT-022)
                command_jump(hwnd, 1);
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
    // Ctrl font zoom (a11y "adjustable font size", Ghostty parity). The
    // plain keys, not the shift symbols: plus is 0xBB+shift on US layouts,
    // and ctrl+shift+0xBD (underscore) is already the split-down binding.
    // Outer None = not a zoom key: fall through so other Ctrl chords reach
    // the VT encoder (Ctrl+C etc.).
    if mods.ctrl && !mods.alt {
        let zoom = match vk {
            0x6B => Some(Some(1.0)),                 // numpad +
            0xBB if mods.shift => Some(Some(1.0)),   // ctrl + shift + = (plus)
            0x6D => Some(Some(-1.0)),                // numpad -
            0xBD if !mods.shift => Some(Some(-1.0)), // ctrl + -
            0x60 | 0x30 => Some(None),               // ctrl + 0 / numpad 0
            _ => None,
        };
        if let Some(zoom) = zoom {
            zoom_font(hwnd, zoom);
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
    // Issue #25: VK_PACKET/IME deliver non-BMP characters as two WM_CHAR
    // messages (high then low surrogate); pair them into one character.
    let ch = match code {
        0xD800..=0xDBFF => {
            if let Some(s) = state_mut(hwnd) {
                s.pending_surrogate = Some(code as u16);
            }
            return;
        }
        0xDC00..=0xDFFF => {
            let hi = state_mut(hwnd).and_then(|s| s.pending_surrogate.take());
            let Some(hi) = hi else { return };
            let combined = 0x1_0000 + ((u32::from(hi) - 0xD800) << 10) + (code - 0xDC00);
            char::from_u32(combined)
        }
        _ => {
            if let Some(s) = state_mut(hwnd) {
                s.pending_surrogate = None;
            }
            char::from_u32(code)
        }
    };
    let Some(ch) = ch else { return };
    // Renaming: typed text extends the tab title buffer (TAB-013).
    if let Some(s) = state_mut(hwnd)
        && let Some((_, buf)) = &mut s.renaming
    {
        if !ch.is_control() {
            buf.push(ch);
        }
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
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
    // PALETTE-001: while the palette is open, typed text edits the query.
    if let Some(palette) = &mut s.palette {
        if ch.is_control() {
            return;
        }
        palette.push_char(ch);
        palette.refresh();
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
        return;
    }
    // While search is active, typed text edits the query, not the shell.
    if let Some(search) = &mut s.search {
        if ch.is_control() {
            return;
        }
        search.query.push(ch);
        let tab = s.app.active_tab();
        search.refresh(tab);
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
        search_timer_sync(hwnd);
        return;
    }
    let bytes = keyboard::encode_char(ch, mods);
    let _ = s.app.send_input_active(&bytes);
}

/// WIN-006: WM_IME_STARTCOMPOSITION — move the IME's composition window to
/// the caret so CJK text appears where the user is typing. The composition
/// string itself is shown by the IME's own UI, not drawn in-grid.
/// shortcut: if the system suppresses the IME UI, the in-progress string is
/// invisible; add in-grid underlining if that is ever reported.
fn position_ime_window(hwnd: HWND) {
    let himc = unsafe { ImmGetContext(hwnd) };
    if himc.0.is_null() {
        return;
    }
    if let Some(s) = state_ref(hwnd)
        && let Some(renderer) = &s.renderer
        && let Some((x, y)) = renderer.cursor_dip_position(&s.app)
    {
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32;
        let form = COMPOSITIONFORM {
            dwStyle: CFS_POINT,
            ptCurrentPos: POINT {
                x: dips_to_px(x, dpi) as i32,
                y: dips_to_px(y, dpi) as i32,
            },
            rcArea: RECT::default(),
        };
        let _ = unsafe { ImmSetCompositionWindow(himc, &form) };
    }
    let _ = unsafe { ImmReleaseContext(hwnd, himc) };
}

/// WIN-006: WM_IME_COMPOSITION — commit the string the IME finalised
/// (`GCS_RESULTSTR`) and tell the IME the composition string was consumed.
fn handle_ime_composition(hwnd: HWND, lparam: LPARAM) {
    if (lparam.0 as u32) & GCS_RESULTSTR.0 == 0 {
        return;
    }
    let himc = unsafe { ImmGetContext(hwnd) };
    if himc.0.is_null() {
        return;
    }
    // Two-call pattern: ask for the byte length, then read the UTF-16 units.
    let len = unsafe { ImmGetCompositionStringW(himc, GCS_RESULTSTR, None, 0) };
    if len > 0 {
        let mut buf = vec![0u16; (len as usize).div_ceil(2)];
        let got = unsafe {
            ImmGetCompositionStringW(
                himc,
                GCS_RESULTSTR,
                Some(buf.as_mut_ptr().cast()),
                u32::try_from(buf.len() * 2).unwrap_or(u32::MAX),
            )
        };
        if got > 0 {
            commit_text(hwnd, &keyboard::utf16_to_chars(&buf[..got as usize / 2]));
        }
    }
    let _ = unsafe { ImmNotifyIME(himc, NI_COMPOSITIONSTR, CPS_COMPLETE, 0) };
    let _ = unsafe { ImmReleaseContext(hwnd, himc) };
}

/// Send an IME-committed string where typed text goes (WIN-006): the rename
/// buffer, the search query, or the PTY as UTF-8. Modifier state is not
/// applied: the IME produced the characters, not the physical keys.
fn commit_text(hwnd: HWND, chars: &[char]) {
    if chars.is_empty() {
        return;
    }
    if let Some(s) = state_mut(hwnd) {
        if let Some((_, buf)) = &mut s.renaming {
            buf.extend(chars.iter().copied().filter(|c| !c.is_control()));
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            return;
        }
        if let Some(search) = &mut s.search {
            search
                .query
                .extend(chars.iter().copied().filter(|c| !c.is_control()));
            let tab = s.app.active_tab();
            search.refresh(tab);
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            search_timer_sync(hwnd);
            return;
        }
        if let Some(palette) = &mut s.palette {
            palette
                .query
                .extend(chars.iter().copied().filter(|c| !c.is_control()));
            palette.refresh();
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            return;
        }
        for &ch in chars {
            let bytes = keyboard::encode_char(ch, Modifiers::default());
            let _ = s.app.send_input_active(&bytes);
        }
    }
}

/// Ctrl+F: open the search overlay, or close it when already open
/// (SEARCH-002). Search starts with an empty query and no matches.
fn toggle_search(hwnd: HWND) {
    let Some(s) = state_mut(hwnd) else { return };
    if s.search.is_some() {
        s.search = None;
    } else if s.cfg.search.visible {
        // `[search]` sets the starting options (SEARCH-007/008/009).
        let defaults = s.cfg.search.clone();
        s.search = Some(SearchState::with_defaults(
            defaults.case_sensitive_default,
            defaults.whole_word_default,
            defaults.regex_default,
        ));
    }
    s.suppress_char = true;
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// PALETTE-001 (issue #39): open the command palette, or close it when
/// already open. The palette takes the overlay strip from search (one
/// overlay per window) and receives every keystroke.
fn toggle_palette(hwnd: HWND) {
    let Some(s) = state_mut(hwnd) else { return };
    if s.palette.is_some() {
        s.palette = None;
    } else {
        s.search = None;
        let items = build_palette_items(s);
        let recent = s.recent.clone();
        s.palette = Some(PaletteState::new(items, &recent));
    }
    s.suppress_char = true;
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Build the command registry (issue #39: one registry drives palette,
/// menus and keybindings). Every [`KeyAction`] is listed, plus "New Tab:
/// <profile>" per profile and "Switch to Tab N" per tab. Disabled rows
/// carry the reason the palette shows for them.
fn build_palette_items(s: &WindowState) -> Vec<PaletteItem> {
    let mut items: Vec<PaletteItem> = Vec::new();
    let pane_count = s.app.active_tab().map_or(0, |tab| tab.pane_count());
    let has_commands = s
        .app
        .active_tab()
        .is_some_and(|tab| !tab.pane().commands().is_empty());
    let last_finished = s
        .app
        .active_tab()
        .is_some_and(|tab| tab.pane().commands().last_finished().is_some());
    // Per-tab and per-profile rows are built before the `push` closure
    // borrows `items` mutably.
    for (idx, tab) in s.app.tabs().iter().enumerate() {
        let title: String = tab.title().chars().take(48).collect();
        items.push(PaletteItem {
            label: format!("Switch to Tab {}: {title}", idx + 1),
            group: "Tabs",
            shortcut: Some(format!("Ctrl+Alt+{}", idx + 1)),
            disabled_reason: (idx == s.app.active_index())
                .then(|| "This tab is already active".to_string()),
            command: Command::SwitchTab(idx),
        });
    }
    for (idx, profile) in s.profiles.iter().enumerate() {
        items.push(PaletteItem {
            label: format!("New Tab: {}", profile.name),
            group: "Profiles",
            shortcut: None,
            disabled_reason: None,
            command: Command::NewTabProfile(idx),
        });
    }
    for rec in live_sessions() {
        let alias = rec
            .get("alias")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let name = rec
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("session");
        items.push(PaletteItem {
            label: format!("View Session {alias}: {name}"),
            group: "Sessions",
            shortcut: None,
            disabled_reason: None,
            command: Command::AttachSession(u32::try_from(alias).unwrap_or(0)),
        });
    }
    let mut push = |label: &str, group: &'static str, action: KeyAction, disabled: Option<&str>| {
        items.push(PaletteItem {
            label: label.to_string(),
            group,
            shortcut: shortcut_for(s, action),
            disabled_reason: disabled.map(str::to_owned),
            command: Command::Action(action),
        });
    };
    push("New Tab", "Shell", KeyAction::NewTab, None);
    push("Close Pane/Tab", "Shell", KeyAction::CloseTab, None);
    push("Copy", "Shell", KeyAction::Copy, None);
    push("Paste", "Shell", KeyAction::Paste, None);
    push("Search", "Shell", KeyAction::Search, None);
    push("Clear Screen", "Shell", KeyAction::ClearScreen, None);
    push(
        "Copy Last Command Output",
        "Shell",
        KeyAction::CopyLastCommand,
        (!last_finished).then_some("No finished command in this pane yet"),
    );
    push(
        "Previous Command",
        "Shell",
        KeyAction::PrevCommand,
        (!has_commands).then_some("No command boundaries in this pane yet"),
    );
    push(
        "Next Command",
        "Shell",
        KeyAction::NextCommand,
        (!has_commands).then_some("No command boundaries in this pane yet"),
    );
    push("Split Right", "Splits", KeyAction::SplitRight, None);
    push("Split Below", "Splits", KeyAction::SplitDown, None);
    let no_split = (pane_count < 2).then_some("This tab has one pane");
    push("Focus Pane Up", "Splits", KeyAction::FocusUp, no_split);
    push("Focus Pane Down", "Splits", KeyAction::FocusDown, no_split);
    push("Focus Pane Left", "Splits", KeyAction::FocusLeft, no_split);
    push(
        "Focus Pane Right",
        "Splits",
        KeyAction::FocusRight,
        no_split,
    );
    push("Resize Pane Up", "Splits", KeyAction::ResizeUp, no_split);
    push(
        "Resize Pane Down",
        "Splits",
        KeyAction::ResizeDown,
        no_split,
    );
    push(
        "Resize Pane Left",
        "Splits",
        KeyAction::ResizeLeft,
        no_split,
    );
    push(
        "Resize Pane Right",
        "Splits",
        KeyAction::ResizeRight,
        no_split,
    );
    push("Rename Tab", "Tabs", KeyAction::RenameTab, None);
    push("Next Tab", "Tabs", KeyAction::NextTab, None);
    push("Previous Tab", "Tabs", KeyAction::PrevTab, None);
    push("Move Tab Left", "Tabs", KeyAction::MoveTabLeft, None);
    push("Move Tab Right", "Tabs", KeyAction::MoveTabRight, None);
    push("Next Profile", "Profiles", KeyAction::NextProfile, None);
    push("Previous Profile", "Profiles", KeyAction::PrevProfile, None);
    push("Next Theme", "Theme", KeyAction::NextTheme, None);
    push("Previous Theme", "Theme", KeyAction::PrevTheme, None);
    push(
        "Increase Font Size",
        "View",
        KeyAction::IncreaseFontSize,
        None,
    );
    push(
        "Decrease Font Size",
        "View",
        KeyAction::DecreaseFontSize,
        None,
    );
    push("Reset Font Size", "View", KeyAction::ResetFontSize, None);
    push("Toggle Fullscreen", "Window", KeyAction::Fullscreen, None);
    push(
        "Hide Quick Terminal",
        "Window",
        KeyAction::CloseQuick,
        (!s.quick).then_some("This is not the quick terminal"),
    );
    push("Reload Config", "Config", KeyAction::ReloadConfig, None);
    push("Quit", "Window", KeyAction::Quit, None);
    items
}

/// The shortcut to display for `action`: a user binding wins, then the
/// built-in default (issue #39: the table drives the display).
fn shortcut_for(s: &WindowState, action: KeyAction) -> Option<String> {
    let spec = s
        .cfg
        .keymap()
        .into_iter()
        .find(|(_, a)| *a == action)
        .map(|(k, _)| k)
        .or_else(|| builtin_shortcut(action).map(str::to_owned))?;
    Some(display_shortcut(&spec))
}

/// Palette-mode key handling. Returns `true` when the key was consumed;
/// printable keys are left to `WM_CHAR`, which edits the query.
fn handle_palette_keydown(hwnd: HWND, vk: u16) -> bool {
    let consumed = match vk {
        0x1B => {
            if let Some(s) = state_mut(hwnd) {
                s.palette = None;
            }
            true
        }
        0x26 => step_palette(hwnd, -1),
        0x28 => step_palette(hwnd, 1),
        0x0D | 0x0A => {
            execute_palette_current(hwnd);
            true
        }
        0x08 => {
            if let Some(s) = state_mut(hwnd)
                && let Some(palette) = &mut s.palette
            {
                palette.backspace();
                palette.refresh();
            }
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

fn step_palette(hwnd: HWND, delta: i32) -> bool {
    if let Some(s) = state_mut(hwnd)
        && let Some(palette) = &mut s.palette
    {
        palette.step(delta);
        true
    } else {
        false
    }
}

/// Run the selected command: close the palette, record the recents entry,
/// then dispatch (the action may destroy the window, so take everything
/// before it runs).
fn execute_palette_current(hwnd: HWND) {
    let (command, label) = {
        let Some(s) = state_mut(hwnd) else { return };
        let Some(item) = s.palette.as_ref().and_then(PaletteState::selected) else {
            return;
        };
        if item.disabled_reason.is_some() {
            return;
        }
        (item.command, item.label.clone())
    };
    if let Some(s) = state_mut(hwnd) {
        s.palette = None;
        PaletteState::record_recent(&mut s.recent, &label);
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    match command {
        Command::Action(action) => run_key_action(hwnd, action),
        Command::NewTabProfile(idx) => open_profile_tab(hwnd, idx),
        Command::AttachSession(alias) => open_session_view(hwnd, alias, false),
        Command::SwitchTab(idx) => {
            if state_mut(hwnd).is_some_and(|s| s.app.select_tab(idx)) {
                sync_active_tab_geometry(hwnd);
                if let Some(s) = state_ref(hwnd) {
                    set_title(hwnd, &s.app);
                }
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                search_resync(hwnd);
            }
        }
    }
}
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
    {
        let Some(s) = state_mut(hwnd) else { return };
        let Some(search) = &mut s.search else { return };
        let tab = s.app.active_tab();
        match kind {
            SearchToggle::Case => search.toggle_case(tab),
            SearchToggle::Word => search.toggle_whole_word(tab),
            SearchToggle::Regex => search.toggle_regex(tab),
        }
    }
    search_timer_sync(hwnd);
}

fn search_backspace(hwnd: HWND) {
    {
        let Some(s) = state_mut(hwnd) else { return };
        let Some(search) = &mut s.search else { return };
        let tab = s.app.active_tab();
        search.backspace(tab);
    }
    search_timer_sync(hwnd);
}

/// Arm or disarm the scan timer (issue #42) to match `search.scanning`.
/// Call after every `refresh`/resync so a bounded scan keeps streaming.
fn search_timer_sync(hwnd: HWND) {
    let scanning = state_ref(hwnd)
        .and_then(|s| s.search.as_ref())
        .is_some_and(|search| search.scanning);
    if scanning {
        let _ = unsafe { SetTimer(Some(hwnd), SEARCH_TIMER, SEARCH_SCAN_MS, None) };
    } else {
        let _ = unsafe { KillTimer(Some(hwnd), SEARCH_TIMER) };
    }
}

/// WM_TIMER: run one bounded search scan chunk (issue #42), repaint only
/// when the result changed, and disarm the timer once the scan finishes.
fn search_step_tick(hwnd: HWND) {
    let Some(s) = state_mut(hwnd) else { return };
    let Some(search) = &mut s.search else {
        let _ = unsafe { KillTimer(Some(hwnd), SEARCH_TIMER) };
        return;
    };
    let tab = s.app.active_tab();
    let changed = search.step(tab);
    if !search.scanning {
        let _ = unsafe { KillTimer(Some(hwnd), SEARCH_TIMER) };
    }
    if changed {
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    }
}

/// The active tab changed (issue #42): an open search re-scans the new
/// tab so results never point at the previous tab's lines.
fn search_resync(hwnd: HWND) {
    {
        let Some(s) = state_mut(hwnd) else { return };
        let Some(search) = &mut s.search else { return };
        let tab = s.app.active_tab();
        search.refresh(tab);
    }
    search_timer_sync(hwnd);
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
        pane.update_follow();
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
        let cell = cell_at(s, pos.0, pos.1, client_height_dips(hwnd));
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
        // Wheel-up (positive delta) is the wheel-up button code (64);
        // wheel-down is 65. The 16-bit delta must be sign-extended.
        let bytes =
            mouse_report_bytes(if delta > 0 { 64 } else { 65 }, col, row, false, sgr, urxvt);
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
        if delta > 0 {
            grid.scroll_up(lines);
        } else {
            grid.scroll_down(lines);
        }
        pane.update_follow();
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

/// Client-area height in DIPs (the tab bar can sit at the bottom, TAB-022).
fn client_height_dips(hwnd: HWND) -> f32 {
    let mut r = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut r) };
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32;
    px_to_dips((r.bottom - r.top).max(1) as u32, dpi)
}

/// Explorer file drop (issue #40, DRAG-001..003). Dropped over the tab bar,
/// each dropped folder opens a new tab in that folder; dropped over the
/// grid, the paths are inserted at the shell prompt quoted for the shell
/// family that will receive them (Git Bash, PowerShell, CMD, WSL).
fn handle_drop(hwnd: HWND, hdrop: HDROP) {
    // Read the drop point and every path before releasing the HDROP.
    let mut pt = POINT::default();
    let _ = unsafe { DragQueryPoint(hdrop, &mut pt) };
    let count = unsafe { DragQueryFileW(hdrop, 0xFFFF_FFFF, None) };
    let mut paths: Vec<String> = Vec::new();
    for i in 0..count {
        let mut buf = [0u16; 2048];
        let len = unsafe { DragQueryFileW(hdrop, i, Some(&mut buf)) };
        if len > 0 {
            paths.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
    }
    unsafe { DragFinish(hdrop) };
    if paths.is_empty() {
        return;
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32;
    let x = px_to_dips(pt.x.max(0) as u32, dpi);
    let y = px_to_dips(pt.y.max(0) as u32, dpi);
    let (over_bar, pane_idx, family, base_spec) = {
        let Some(s) = state_ref(hwnd) else { return };
        let height = client_height_dips(hwnd);
        let bar = s.bar_height();
        let top = s.bar_top(height);
        let over_bar = bar > 0.0 && y >= top && y < top + bar;
        let pane_idx = if over_bar {
            None
        } else {
            cell_at(s, x, y, height).map(|(idx, _, _)| idx)
        };
        // The tab's profile names the spec; the spec's program decides
        // the quoting rules and the cwd the new tab clones.
        let spec = s
            .app
            .active_tab()
            .and_then(|t| {
                let name = t.profile().to_string();
                s.profiles.iter().find(|p| p.name == name).cloned()
            })
            .or_else(|| s.profiles.first().cloned());
        let family = spec
            .as_ref()
            .map_or(ShellFamily::Other, |p| shell_family(&p.program));
        (over_bar, pane_idx, family, spec)
    };
    if over_bar {
        // Each dropped folder opens a tab at that directory (Open
        // terminal here, same mechanism as the CLI positional arg).
        let Some(base) = base_spec else { return };
        for dir in paths.iter().filter(|p| std::path::Path::new(p).is_dir()) {
            let mut spec = base.clone();
            spec.cwd = Some(dir.clone());
            new_tab_with_profile(hwnd, spec);
        }
        return;
    }
    let Some(idx) = pane_idx else { return };
    // `quote_for_drop` returns one space-separated quoted path per file.
    let text: String = paths
        .iter()
        .map(|p| quote_for_drop(family, p))
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(s) = state_mut(hwnd) {
        let _ = s.app.send_input_pane(idx, text.as_bytes());
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Grid cell under a client-area DIP position, addressed by pane:
/// `(pane flatten index, viewport row, viewport col)` inside that pane;
/// `None` for the tab bar. Clamped to the grid bounds so drags past the
/// edges extend the selection (WIN-004, SPLIT-004 hit-testing).
fn cell_at(s: &WindowState, x: f32, y: f32, height: f32) -> Option<(usize, usize, usize)> {
    let top = s.grid_top();
    let bottom = top + (height - s.bar_height() - 2.0 * s.pad_y()).max(0.0);
    if y < top || y >= bottom {
        return None;
    }
    let tab = s.app.active_tab()?;
    let col = ((x - s.pad_x()) / s.metrics.cell_width).floor().max(0.0) as usize;
    let row = ((y - top) / s.metrics.cell_height).floor().max(0.0) as usize;
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
    let height = client_height_dips(hwnd);
    state_ref(hwnd).and_then(|s| cell_at(s, x, y, height))
}

/// OSC 8 link target of grid cell (`view_row`, `col`) in pane `idx`
/// (view coordinates); `None` for non-link cells and for targets
/// evicted from the parser's bounded URI table (issue #41).
fn link_uri_at(hwnd: HWND, idx: usize, view_row: usize, col: usize) -> Option<String> {
    state_ref(hwnd).and_then(|s| {
        let pane = s.app.active_tab()?.pane_at(idx)?;
        let id = pane.grid().row(view_row)?.cells.get(col)?.link;
        (id != 0)
            .then(|| pane.parser().link_uri(id).map(str::to_string))
            .flatten()
    })
}

/// SEC-004: only `http`/`https` targets open automatically; everything
/// else (file:, mailto:, evicted) is copy-only.
fn openable_uri(uri: &str) -> bool {
    uri.starts_with("http://") || uri.starts_with("https://")
}

/// SEC-003: this runs only from an explicit Ctrl+click or the context
/// menu's Open Link, never from output.
fn open_link(hwnd: HWND, uri: &str) {
    if !openable_uri(uri) {
        return;
    }
    let op = wstr("open");
    let url: Vec<u16> = uri.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = ShellExecuteW(
            Some(hwnd),
            PCWSTR(op.as_ptr()),
            PCWSTR(url.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOW,
        );
    }
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
    // handle_rbutton_down passes LPARAM(0): use the real cursor so the
    // tab-bar hit test below sees the click where it actually happened.
    if pt.x < 0 || pt.y < 0 || (pt.x == 0 && pt.y == 0) {
        let _ = unsafe { GetCursorPos(&mut pt) };
    }
    // Right-click on the tab-bar strip: tab popup (rename/close) or the
    // profile menu on `+`, not the grid menu.
    let mut cpt = pt;
    if unsafe { ScreenToClient(hwnd, &mut cpt) }.as_bool() {
        let lp = LPARAM((((cpt.y as u32 & 0xFFFF) << 16) | (cpt.x as u32 & 0xFFFF)) as isize);
        let (x, y) = cursor_dips(hwnd, lp);
        let hit = state_ref(hwnd).map(|s| s.tab_hit(x, y, client_height_dips(hwnd)));
        match hit {
            Some(TabHit::Tab(i)) | Some(TabHit::Close(i)) => {
                show_tab_menu(hwnd, i);
                return;
            }
            Some(TabHit::Plus) => {
                show_profile_menu(hwnd);
                return;
            }
            _ => {}
        }
    }
    // Issue #41: OSC 8 link under the pointer gets Open/Copy Link items.
    let link_uri = {
        let lp = LPARAM((((cpt.y as u32 & 0xFFFF) << 16) | (cpt.x as u32 & 0xFFFF)) as isize);
        let (x, y) = cursor_dips(hwnd, lp);
        state_ref(hwnd).and_then(|s| {
            let (idx, row, col) = cell_at(s, x, y, client_height_dips(hwnd))?;
            let pane = s.app.active_tab()?.pane_at(idx)?;
            let id = pane.grid().row(row)?.cells.get(col)?.link;
            if id == 0 {
                return None;
            }
            pane.parser().link_uri(id).map(str::to_string)
        })
    };
    unsafe {
        let menu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return,
        };
        if let Some(uri) = &link_uri {
            let _ = append_menu_item(menu, "Copy Link", MENU_COPY_LINK);
            if openable_uri(uri) {
                let _ = append_menu_item(menu, "Open Link	Ctrl+Click", MENU_OPEN_LINK);
            }
            let _ = append_menu_separator(menu);
        }
        let _ = append_menu_item(menu, "Copy	Ctrl+Shift+C", MENU_COPY);
        let _ = append_menu_item(menu, "Paste	Ctrl+Shift+V", MENU_PASTE);
        let _ = append_menu_separator(menu);
        let _ = append_menu_item(menu, "Search	Ctrl+F", MENU_SEARCH);
        let _ = append_menu_item(menu, "Split Right	Ctrl+Shift+Enter", MENU_SPLIT_RIGHT);
        let _ = append_menu_item(menu, "Split Below	Ctrl+Shift+-", MENU_SPLIT_DOWN);
        let _ = append_menu_separator(menu);
        let _ = append_menu_item(menu, "New Tab	Ctrl+Shift+T", MENU_NEW_TAB);
        let _ = append_menu_item(menu, "Close Tab	Ctrl+Shift+W", MENU_CLOSE_TAB);
        if let Some(uri) = link_uri
            && let Some(s) = state_mut(hwnd)
        {
            s.link_target = Some(uri);
        }
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        // Menu dismissed with no Open/Copy selection: drop the target.
        if let Some(s) = state_mut(hwnd) {
            s.link_target = None;
        }
        let _ = DestroyMenu(menu);
    }
}

/// `+` click: native dropdown of the shell profiles (Windows-Terminal
/// style). Reuses the File > Profiles command ids, so selection dispatches
/// through `handle_command` into `open_profile_tab`.
fn show_profile_menu(hwnd: HWND) {
    let Some(names) = state_ref(hwnd).map(|s| {
        s.profiles
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>()
    }) else {
        return;
    };
    if names.is_empty() {
        return;
    }
    let mut pt = POINT::default();
    if unsafe { GetCursorPos(&mut pt) }.is_err() {
        return;
    }
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        for (idx, name) in names.iter().enumerate() {
            let _ = append_menu_item(menu, name, MENU_PROFILE_BASE + idx);
        }
        // The popup runs a modal loop that re-enters the window proc.
        let _ = SetForegroundWindow(hwnd);
        let sel = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_RETURNCMD | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        // The documented follow-up so the next click is not swallowed.
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        if sel.0 > 0 {
            handle_command(hwnd, WPARAM(sel.0 as usize));
        }
    }
}

/// Right-click on a tab: native popup targeting that tab. The tab is
/// selected first so Rename/Close apply to it.
fn show_tab_menu(hwnd: HWND, idx: usize) {
    if let Some(s) = state_mut(hwnd)
        && s.app.select_tab(idx)
    {
        apply_active_profile_theme(s);
        set_title(hwnd, &s.app);
    }
    sync_active_tab_geometry(hwnd);
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    search_resync(hwnd);
    let mut pt = POINT::default();
    if unsafe { GetCursorPos(&mut pt) }.is_err() {
        return;
    }
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        let _ = append_menu_item(menu, "&Rename Tab", MENU_RENAME_TAB);
        let is_view = state_ref(hwnd).is_some_and(|s| {
            s.app
                .active_tab()
                .is_some_and(|t| t.pane().view_alias().is_some())
        });
        if is_view {
            let _ = append_menu_item(menu, "Copy Attach Command\t", MENU_VIEW_COPY_ATTACH);
            let _ = append_menu_item(menu, "Take Control", MENU_VIEW_TAKE_CONTROL);
            let _ = append_menu_item(menu, "Terminate Session...", MENU_VIEW_TERMINATE);
        }
        let _ = append_menu_item(menu, "Close Tab\tCtrl+Shift+W", MENU_CLOSE_TAB);
        let _ = SetForegroundWindow(hwnd);
        let sel = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_RETURNCMD | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        if sel.0 > 0 {
            handle_command(hwnd, WPARAM(sel.0 as usize));
        }
    }
}

fn handle_lbutton_down(hwnd: HWND, lparam: LPARAM) {
    let (x, y) = cursor_dips(hwnd, lparam);
    let hit = state_ref(hwnd).map(|s| s.tab_hit(x, y, client_height_dips(hwnd)));
    match hit {
        Some(TabHit::Tab(i)) => {
            if let Some(s) = state_mut(hwnd) {
                // Press-and-drag reorders the tab (TAB-016).
                s.tab_drag = Some(i);
                if s.app.select_tab(i) {
                    apply_active_profile_theme(s);
                }
                set_title(hwnd, &s.app);
            }
            sync_active_tab_geometry(hwnd); // issue #9
            unsafe { SetCapture(hwnd) };
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            search_resync(hwnd);
            return;
        }
        Some(TabHit::Close(i)) => {
            close_tab(hwnd, i);
            return;
        }
        Some(TabHit::Plus) => {
            show_profile_menu(hwnd);
            return;
        }
        Some(TabHit::None) | None => {}
    }
    if state_ref(hwnd).is_some_and(|s| s.tab_hit(x, y, client_height_dips(hwnd)) != TabHit::None) {
        return;
    }
    let height = client_height_dips(hwnd);
    let anchor = state_ref(hwnd).and_then(|s| {
        let (idx, row, col) = cell_at(s, x, y, height)?;
        let abs = abs_row_in_pane(s, idx, row);
        Some((idx, row, col, abs))
    });
    let Some((idx, view_row, col, abs)) = anchor else {
        return;
    };
    let a = Pos { row: abs, col };
    // Clicking a pane focuses it (SPLIT-003); the keystrokes and the
    // follow-up selection drag then belong to that pane.
    if let Some(s) = state_mut(hwnd) {
        s.app.focus_pane(idx);
    }
    if state_ref(hwnd).is_some_and(|s| mouse_protocol_at(s, idx) != MouseProtocol::Off) {
        send_mouse_report(hwnd, idx, a.row, a.col, 0, false, false);
        return;
    }
    // Issue #41: Ctrl+click on an OSC 8 link opens the target (SEC-003:
    // explicit user interaction); no selection is started.
    if unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0
        && let Some(uri) = link_uri_at(hwnd, idx, view_row, col)
    {
        open_link(hwnd, &uri);
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
    let height = client_height_dips(hwnd);
    let cell = state_ref(hwnd).and_then(|s| {
        let (idx, row, col) = cell_at(s, x, y, height)?;
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
    let height = client_height_dips(hwnd);
    // Tab drag reorder (TAB-016): drop the dragged tab on the tab it crosses.
    if let Some(from) = state_ref(hwnd).and_then(|s| s.tab_drag)
        && let Some(to) = state_ref(hwnd).map(|s| s.tab_hit(x, y, height))
    {
        if let TabHit::Tab(idx) = to
            && idx != from
            && state_mut(hwnd).is_some_and(|s| {
                let moved = s.app.move_tab(from, idx);
                if moved {
                    s.tab_drag = Some(idx);
                }
                moved
            })
        {
            if let Some(s) = state_ref(hwnd) {
                set_title(hwnd, &s.app);
            }
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
        }
        return;
    }
    update_tab_marquee(hwnd, x, y, height);
    if let Some((idx, row, col)) = state_ref(hwnd).and_then(|s| cell_at(s, x, y, height))
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
    // Dragging over the tab bar clamps the selection to the grid edge.
    let height = client_height_dips(hwnd);
    let top = s.grid_top();
    let bottom = top + (height - s.bar_height()).max(0.0);
    let y = y.clamp(top, (bottom - 0.001).max(top));
    let Some((_, row, col)) = cell_at(s, x, y, height) else {
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

/// Signboard marquee: while the mouse rests on a tab whose title is cut
/// off, scroll the full title through it (a timer drives the frames).
fn update_tab_marquee(hwnd: HWND, x: f32, y: f32, height: f32) {
    let target = match state_ref(hwnd).map(|s| s.tab_hit(x, y, height)) {
        Some(TabHit::Tab(i)) => Some(i),
        _ => None,
    };
    let Some(s) = state_mut(hwnd) else {
        return;
    };
    let Some(r) = s.renderer.as_mut() else {
        return;
    };
    let target = target.filter(|i| r.title_overflows(*i));
    if r.marquee_tab() == target {
        return;
    }
    r.set_marquee(target);
    if target.is_some() {
        let _ = unsafe { SetTimer(Some(hwnd), MARQUEE_TIMER, MARQUEE_MS, None) };
    } else {
        let _ = unsafe { KillTimer(Some(hwnd), MARQUEE_TIMER) };
    }
    invalidate_tab_bar(hwnd);
}

/// Repaint only the tab-bar strip: the marquee animation lives there, and
/// repainting the grid every 50 ms is what makes the ticker feel laggy.
fn invalidate_tab_bar(hwnd: HWND) {
    let mut rc = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut rc) };
    let scale = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0;
    let bottom = state_ref(hwnd).map_or(1.0, |s| s.grid_top()).max(1.0) * scale;
    let bar = RECT {
        left: 0,
        top: 0,
        right: rc.right,
        bottom: bottom.ceil() as i32,
    };
    let _ = unsafe { InvalidateRect(Some(hwnd), Some(&bar), false) };
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
    let (x, y) = cursor_dips(hwnd, lparam);
    let copy_link = state_ref(hwnd).and_then(|s| {
        // Issue #41: a plain click (no drag) on an OSC 8 link copies its
        // target. The drag anchor is still set here, so a real selection
        // drag is excluded by the cell-equality check.
        let _ = s.drag_anchor?; // a real click armed the drag anchor
        let (idx, row, col) = cell_at(s, x, y, client_height_dips(hwnd))?;
        let end = s.app.active_tab()?.pane_at(idx)?.selection()?.end;
        if end.row != abs_row_in_pane(s, idx, row) || end.col != col {
            return None; // a drag ended elsewhere; not a plain click
        }
        let pane = s.app.active_tab()?.pane_at(idx)?;
        let id = pane.grid().row(row)?.cells.get(col)?.link;
        if id == 0 {
            return None;
        }
        pane.parser().link_uri(id).map(str::to_string)
    });
    if let Some(uri) = copy_link {
        let _ = clipboard::set_text(&uri);
    }
    if let Some(s) = state_mut(hwnd) {
        s.drag_anchor = None;
        s.tab_drag = None;
    }
    // CONFIG-010: copy-on-select puts the selection on the clipboard.
    if state_ref(hwnd).is_some_and(|s| s.cfg.copy_on_select) {
        copy_selection(hwnd);
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
    let over_bar = state_ref(target).is_some_and(|s| s.over_tab_bar(y, client_height_dips(target)));
    let idc = if over_bar { IDC_ARROW } else { IDC_IBEAM };
    if let Ok(cursor) = unsafe { LoadCursorW(None, idc) } {
        let _ = unsafe { SetCursor(Some(cursor)) };
    }
}

/// Ctrl+Shift+]/[: move the active tab by `delta` (wrapping).
fn switch_tab_rel(hwnd: HWND, delta: i32) {
    {
        let Some(s) = state_mut(hwnd) else { return };
        let n = s.app.tab_count();
        if n < 2 {
            return;
        }
        let idx = ((s.app.active_index() as i32 + delta).rem_euclid(n as i32)) as usize;
        s.app.select_tab(idx);
    }
    sync_active_tab_geometry(hwnd); // issue #9
    if let Some(s) = state_ref(hwnd) {
        set_title(hwnd, &s.app);
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    search_resync(hwnd);
}

/// F11: toggle borderless fullscreen over the nearest monitor,
/// restoring the saved window rect and styles on the second press.
fn toggle_fullscreen(hwnd: HWND) {
    // GHSA-mw68: SetWindowPos dispatches WM_SIZE/WM_PAINT synchronously,
    // which re-enters the window state. Take/return the slot around the
    // calls; never hold the state borrow across them.
    let saved = state_mut(hwnd).and_then(|s| s.fullscreen.take());
    if let Some((rect, style, exstyle)) = saved {
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
        let entered = unsafe {
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
                true
            } else {
                false
            }
        };
        if entered && let Some(s) = state_mut(hwnd) {
            s.fullscreen = Some((rect, style, exstyle));
        }
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Escape the five XML metacharacters: the toast body carries shell text.
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// BELL-005: a Windows toast, attributed to rustty by the AUMID.
fn show_toast(title: &str, body: &str) {
    let xml = format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        xml_escape(title),
        xml_escape(body)
    );
    let _ = (|| -> Result<()> {
        // SAFETY: FFI boundary (NFR-401) - the AUMID is a static wide string.
        let id = aumid();
        unsafe {
            let _ = SetCurrentProcessExplicitAppUserModelID(&id);
        }
        let doc = XmlDocument::new()?;
        doc.LoadXml(&windows::core::HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        ToastNotificationManager::CreateToastNotifierWithId(&id)?.Show(&toast)
    })();
}

/// WM_TIMER: drain every shell, close dead panes (and the tabs they leave
/// behind), refresh the caption.
fn drain_and_cleanup(hwnd: HWND) {
    // Scope the GWLP_USERDATA borrow so it ends before sync_tab_bar re-enters.
    let mut history_scan = false;
    let changed = {
        let Some(s) = state_mut(hwnd) else { return };
        s.app.drain_shells();
        s.take_bell(hwnd);
        s.notify_finished_command(hwnd);
        s.watch_config(hwnd);
        // Repaint only when a grid actually changed (NFR-113: no idle
        // repaints), and only the rows the grid wrote: paint the damage
        // directly instead of invalidating the client area, so a burst of
        // shell output costs one partial frame per tick (NFR-113/114).
        if s.app.take_dirty() {
            history_scan = s
                .search
                .as_mut()
                .is_some_and(|search| search.note_history(s.app.active_tab()));
            if let Some(r) = &mut s.renderer {
                let prompt = s
                    .renaming
                    .as_ref()
                    .map(|(_, buf)| format!("Rename tab: {buf}  (Enter to accept, Esc to cancel)"));
                let active = s.app.active_index();
                r.set_bell_state(
                    s.bell_flash,
                    &s.bell_tabs,
                    s.bell_tabs.get(active).copied().unwrap_or(false),
                );
                r.paint_damage(
                    &s.app,
                    &s.theme,
                    s.search.as_ref(),
                    s.palette.as_ref(),
                    prompt.as_deref(),
                );
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
    if history_scan {
        search_timer_sync(hwnd);
    }
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
        eprintln!("rustty: split shell spawn failed: {e}");
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Ctrl+Shift+W: close the focused pane when the tab is split (SPLIT-006,
/// with a confirm box when its shell is live); otherwise close the tab,
/// and closing the last tab closes the window (WM_DESTROY reclaims the
/// state and quits the loop).
/// TAB-014/021: a native yes/no box. The default button is "No", so a
/// stray Enter keeps the shells alive.
fn confirm_yes_no(hwnd: HWND, text: &str) -> bool {
    let text = wstr(text);
    let caption = wstr("rustty");
    let answer = unsafe {
        MessageBoxW(
            Some(hwnd),
            PCWSTR::from_raw(text.as_ptr()),
            PCWSTR::from_raw(caption.as_ptr()),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        )
    };
    answer == IDYES
}

fn close_pane_or_tab(hwnd: HWND) {
    let (multi, live, busy, confirm) = match state_ref(hwnd) {
        Some(s) => (
            s.app.pane_count() > 1,
            s.app
                .active_tab()
                .is_some_and(|t| t.pane().shell().state() == crate::shell::ShellState::Running),
            // TAB-018 "shell is busy": OSC 133 reported a command start and
            // no prompt has come back yet.
            s.app
                .active_tab()
                .is_some_and(|t| t.pane().command_running()),
            s.app.tab_confirm_close(),
        ),
        None => return,
    };
    // TAB-018 / SPLIT-006: confirm when the profile asks and the close would
    // kill a running shell, or would quit (it is the last tab).
    if confirm && (live || !multi) {
        let text = if busy {
            "A command is running in this tab. Close it and kill the shell?"
        } else {
            "Close this tab? It is the last one, so rustty will quit."
        };
        if !confirm_yes_no(hwnd, text) {
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

/// Ctrl+Shift+T: append a tab for the selected profile.
fn new_tab(hwnd: HWND) {
    let spec = state_ref(hwnd).map(active_profile);
    if let Some(spec) = spec {
        new_tab_with_profile(hwnd, spec);
    }
}

/// Append a tab for `spec`: profile label, tab colour (TAB-015), per-profile
/// theme (THEME-011) and scrollback, then spawn its shell (SHELL-017..021).
fn new_tab_with_profile(hwnd: HWND, spec: ProfileSpec) {
    let Some(s) = state_mut(hwnd) else {
        return;
    };
    let (cols, rows) = s
        .app
        .active_tab()
        .map_or((s.spawn.cols, s.spawn.rows), |t| (t.cols(), t.rows()));
    let scrollback = spec.scrollback.unwrap_or(s.cfg.scrollback);
    s.app.add_tab_profile(
        cols.max(MIN_COLS),
        rows.max(MIN_ROWS),
        scrollback,
        spec.name.clone(),
        spec.color,
    );
    apply_profile_theme(s, &spec);
    apply_profile_font(s, &spec);
    if let Some(tab) = s.app.active_tab_mut() {
        tab.set_confirm(spec.confirm_close);
    }
    // Auto/hidden tab bars change the strip height when the count changes;
    // sync first so the new shell spawns at the grid the bar leaves room for.
    sync_tab_bar(hwnd);
    let Some(s) = state_mut(hwnd) else { return };
    let (cols, rows) = s
        .renderer
        .as_ref()
        .map_or((s.spawn.cols, s.spawn.rows), |r| r.grid_size());
    let (cols, rows) = (cols.max(MIN_COLS), rows.max(MIN_ROWS));
    // The bar sync above can change the grid the window fits; keep the new
    // tab's grid and its ConPTY on the same size (issue #8).
    if let Some(tab) = s.app.active_tab_mut() {
        let _ = tab.resize(cols, rows);
    }
    let opts = spec_opts(&spec, cols, rows, s.cfg.shell_integration);
    if let Err(e) = s.app.spawn_active(&opts) {
        eprintln!("rustty: shell spawn failed: {e}");
    }
    set_title(hwnd, &s.app);
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Live host sessions for the palette (issue #49). Connecting only (no
/// spawn) keeps the palette inert when no host runs; the client drops
/// here so listing never counts as a host connection.
fn live_sessions() -> Vec<serde_json::Value> {
    let Ok(mut client) = crate::host::client::Client::connect_existing() else {
        return Vec::new();
    };
    let Ok(resp) = client.call("session.list", serde_json::json!({})) else {
        return Vec::new();
    };
    drop(client);
    resp.get("result")
        .and_then(|r| r.get("sessions"))
        .and_then(serde_json::Value::as_array)
        .map(|list| {
            list.iter()
                .filter(|rec| rec.get("state").and_then(serde_json::Value::as_str) == Some("live"))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Open a tab that views live host session `alias` (issue #49). Closing
/// the tab detaches; the session keeps running.
fn open_session_view(hwnd: HWND, alias: u32, take_control: bool) {
    let Some(rec) = live_sessions()
        .into_iter()
        .find(|rec| rec.get("alias").and_then(serde_json::Value::as_u64) == Some(u64::from(alias)))
    else {
        let text = wstr(&format!("Session {alias} is not live."));
        let caption = wstr("rustty");
        unsafe {
            let _ = MessageBoxW(
                Some(hwnd),
                PCWSTR::from_raw(text.as_ptr()),
                PCWSTR::from_raw(caption.as_ptr()),
                MB_OK | MB_ICONASTERISK,
            );
        }
        return;
    };
    let cols = usize::try_from(
        rec.get("cols")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(80),
    )
    .unwrap_or(80)
    .max(MIN_COLS);
    let rows = usize::try_from(
        rec.get("rows")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(24),
    )
    .unwrap_or(24)
    .max(MIN_ROWS);
    let label = format!(
        "Session {alias} · {}",
        if take_control { "ctrl" } else { "watch" }
    );
    let Some(s) = state_mut(hwnd) else { return };
    s.app
        .add_tab_profile(cols, rows, s.cfg.scrollback, label, None);
    // The view's grid starts at the session's recorded geometry; the
    // host owns that geometry and pushes Resize events from there.
    sync_tab_bar(hwnd);
    let Some(s) = state_mut(hwnd) else { return };
    if let Some(tab) = s.app.active_tab_mut() {
        let mut pane = tab.pane_mut();
        if let Err(e) = pane.attach_view(alias, take_control) {
            pane.feed(format!("\r\n[attach failed: {e}]\r\n").as_bytes());
        }
    }
    set_title(hwnd, &s.app);
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Copy `rustty --attach N` for this tab's session view (issue #49).
fn copy_attach_command(hwnd: HWND) {
    let alias =
        state_ref(hwnd).and_then(|s| s.app.active_tab().and_then(|t| t.pane().view_alias()));
    if let Some(alias) = alias {
        let _ = clipboard::set_text(&format!("rustty --attach {alias}"));
    }
}

/// Ask this tab's session view to take controller rights (issue #49).
fn take_view_control(hwnd: HWND) {
    if let Some(s) = state_ref(hwnd)
        && let Some(tab) = s.app.active_tab()
    {
        tab.pane().take_control();
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Terminate this tab's host session after confirmation, then close the
/// tab (issue #49).
fn terminate_view_session(hwnd: HWND) {
    let Some(alias) =
        state_ref(hwnd).and_then(|s| s.app.active_tab().and_then(|t| t.pane().view_alias()))
    else {
        return;
    };
    if !confirm_yes_no(
        hwnd,
        &format!("Terminate session {alias}? The running program is killed."),
    ) {
        return;
    }
    if let Ok(mut client) = crate::host::client::Client::connect_existing() {
        let _ = client.call("session.terminate", serde_json::json!({"alias": alias}));
    }
    let idx = state_ref(hwnd).map(|s| s.app.active_index());
    if let Some(idx) = idx {
        close_tab(hwnd, idx);
    }
}

/// Spawn options from a profile: program, args, directory, environment.
fn spec_opts(spec: &ProfileSpec, cols: usize, rows: usize, integrate: bool) -> SpawnOptions {
    let mut opts = SpawnOptions {
        program: spec.program.clone(),
        args: spec.args.clone(),
        cols,
        rows,
        cwd: spec.cwd.clone(),
        env: spec.env.clone(),
    };
    if integrate {
        opts.apply_shell_integration();
    }
    opts
}

/// The active tab's profile, falling back to the selector
/// (SPLIT-007: a split pane inherits its tab's profile).
fn tab_profile(s: &WindowState) -> ProfileSpec {
    s.app
        .active_tab()
        .and_then(|t| s.profiles.iter().find(|p| p.name == t.profile()).cloned())
        .unwrap_or_else(|| active_profile(s))
}

/// Apply a profile's font (SHELL-019). One renderer serves every tab, so the
/// whole window follows, and the grid the window fits changes with the new
/// metrics: every tab is resized to the new grid.
/// Config-driven chrome the renderer owns: tab bar placement/labels (TAB-022,
/// TAB-023, CONFIG-010), cursor (CONFIG-009), split border and padding, window
/// padding. Startup and CONFIG-018 hot reload share this.
fn sync_chrome(s: &mut WindowState) {
    let Some(r) = &mut s.renderer else {
        return;
    };
    r.set_tab_bar_options(
        s.cfg.tab_bar.placement,
        s.cfg.tab_bar.show_profiles,
        s.cfg.tab_bar.close_button,
    );
    r.set_cursor_options(s.cfg.cursor_shape, s.cfg.cursor_blink);
    r.set_split_options(s.cfg.split.border, s.cfg.split.padding);
    r.set_padding(s.cfg.window.padding.x, s.cfg.window.padding.y);
}

/// CONFIG-018: re-read the config file and apply what can change live. Shells
/// are never restarted; `scrollback` and `starting-profile` only reach the next
/// tab, and DPI stays whatever the manifest said at startup. A config that no
/// longer parses is reported and the running one is kept.
fn reload_config(s: &mut WindowState, hwnd: HWND) {
    let cfg = match Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("rustty: config reload failed: {e}");
            return;
        }
    };
    // THEME-009/010/013/014: the same resolution startup uses, system theme
    // included, so an unset `theme` follows a dark/light flip too.
    let name = if cfg.theme.name().is_empty() {
        theme::Theme::system_preset()
    } else {
        cfg.theme.name()
    };
    let (new_theme, problems) =
        theme::resolve_configured(name, &cfg.theme.overrides(), s.theme_dir.as_deref());
    for problem in &problems {
        eprintln!("rustty: {problem}");
    }
    let font = cfg.font();
    let font_changed = font != s.font;
    s.theme = new_theme;
    s.cfg = cfg;
    // PALETTE-001: shortcuts and profiles changed; rebuild the registry on
    // the next open.
    s.palette = None;
    s.tab_bar = s.cfg.tab_bar.visible;
    s.app.set_show_cwd(s.cfg.tab_bar.show_cwd);
    // Issue #40: re-merge with the current discovery results, then re-run
    // the probes; the WM_APP handler re-merges when fresh results arrive.
    s.profiles = merge_profiles(&s.cfg, &discovered_now());
    start_discovery(hwnd);
    s.profile = s.profile.min(s.profiles.len().saturating_sub(1));
    sync_chrome(s);
    if font_changed {
        // A font the renderer refuses (bad family, metrics failure) keeps the
        // old one; the window never goes blind over a config edit.
        if s.renderer
            .as_mut()
            .is_some_and(|r| r.set_font(&font).is_ok())
        {
            s.font = font;
        }
    }
    // SHELL-019: the active tab's profile overrides the global theme and font.
    let spec = active_profile(s);
    apply_profile_theme(s, &spec);
    apply_profile_font(s, &spec);
    apply_opacity(hwnd);
    sync_tab_bar(hwnd);
    // A new font, bar placement, bar visibility or padding changes how many
    // cells fit in the same client rect, so refit every tab, not just the
    // active one (FONT-002, TAB-022, CONFIG-011).
    let mut rect = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut rect) };
    if let Some(s) = state_mut(hwnd) {
        if let Some(r) = s.renderer.as_mut() {
            r.resize(rect.right.max(0) as u32, rect.bottom.max(0) as u32);
            let (cols, rows) = r.grid_size().max((MIN_COLS, MIN_ROWS));
            if s.app.resize_all_tabs(cols, rows).is_err() {
                eprintln!("rustty: ConPTY resize failed after a config reload");
            }
        }
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    }
}

fn apply_profile_font(s: &mut WindowState, spec: &ProfileSpec) {
    let mut font = s.font.clone();
    let mut changed = false;
    if let Some(family) = spec.font.as_ref() {
        font.family = family.clone();
        changed = true;
    }
    if let Some(size) = spec.font_size.filter(|v| (4.0..=72.0).contains(v)) {
        font.size = size;
        changed = true;
    }
    if let Some(lh) = spec.line_height.filter(|v| (0.8..=2.0).contains(v)) {
        font.line_height = lh;
        changed = true;
    }
    if !changed {
        return;
    }
    let Some(renderer) = s.renderer.as_mut() else {
        s.font = font;
        return;
    };
    if renderer.set_font(&font).is_err() {
        return; // the previous font keeps working, the window is unchanged
    }
    s.font = font;
    let (cols, rows) = renderer.grid_size().max((MIN_COLS, MIN_ROWS));
    if s.app.resize_all_tabs(cols, rows).is_err() {
        eprintln!("rustty: ConPTY resize failed after a profile font change");
    }
}

/// Keyboard font zoom (Ghostty parity: `increase/decrease/reset_font_size`,
/// a11y "adjustable font size"). `Some(step)` nudges the live size by
/// `step` pt inside the config's bounds; `None` restores the configured
/// size. A font the renderer refuses leaves the window unchanged, the same
/// rule as profile fonts (FONT-018).
fn zoom_font(hwnd: HWND, step: Option<f32>) {
    let Some(s) = state_mut(hwnd) else {
        return;
    };
    let mut font = s.font.clone();
    font.size = match step {
        Some(d) => {
            (font.size + d).clamp(crate::config::MIN_FONT_SIZE, crate::config::MAX_FONT_SIZE)
        }
        None => s.cfg.font().size,
    };
    if font.size == s.font.size {
        return;
    }
    let Some(renderer) = s.renderer.as_mut() else {
        s.font = font;
        return;
    };
    if renderer.set_font(&font).is_err() {
        return;
    }
    s.font = font;
    let (cols, rows) = renderer.grid_size().max((MIN_COLS, MIN_ROWS));
    if s.app.resize_all_tabs(cols, rows).is_err() {
        eprintln!("rustty: ConPTY resize failed after a font zoom");
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// THEME-011: a profile's theme is the window theme while its tab lives. The
/// name may be a preset or a `themes/<name>.toml` file.
fn apply_profile_theme(s: &mut WindowState, spec: &ProfileSpec) {
    let Some(name) = spec.theme.as_deref() else {
        return;
    };
    let (theme, problems) =
        theme::resolve_configured(name, &theme::ThemeFile::default(), s.theme_dir.as_deref());
    for problem in &problems {
        eprintln!("rustty: {problem}");
    }
    s.theme = theme;
}

/// The `themes/` directory beside the config file, if the config has a path.
fn theme_dir_from_config() -> Option<std::path::PathBuf> {
    crate::config::config_path().and_then(|p| p.parent().map(|dir| dir.join("themes")))
}

/// THEME-011 for the tab the user just switched to.
fn apply_active_profile_theme(s: &mut WindowState) {
    let spec = tab_profile(s);
    apply_profile_theme(s, &spec);
}

/// TAB-013: start an inline rename of the active tab.
fn start_rename_tab(hwnd: HWND) {
    let Some(s) = state_mut(hwnd) else {
        return;
    };
    let idx = s.app.active_index();
    let title = s
        .app
        .active_tab()
        .map(|t| t.title().to_string())
        .unwrap_or_default();
    s.renaming = Some((idx, title));
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// Enter accepts, Esc cancels, Backspace deletes; printable keys arrive as
/// WM_CHAR and extend the buffer (TAB-013).
fn handle_rename_keydown(hwnd: HWND, vk: u16) -> bool {
    let Some(s) = state_mut(hwnd) else {
        return false;
    };
    let Some((idx, buf)) = s.renaming.take() else {
        return false;
    };
    match vk {
        0x1B => {}
        0x0D => {
            if let Some(tab) = s.app.tab_at_mut(idx) {
                tab.set_title(buf);
            }
            set_title(hwnd, &s.app);
        }
        0x08 => {
            let mut trimmed = buf;
            trimmed.pop();
            s.renaming = Some((idx, trimmed));
        }
        _ => {
            s.renaming = Some((idx, buf));
            return false;
        }
    }
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    true
}

/// TAB-017: keyboard tab reorder.
fn move_active_tab(hwnd: HWND, delta: i32) {
    if state_mut(hwnd).is_some_and(|s| s.app.move_active_tab(delta as isize)) {
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    }
}

/// Close tab `idx` (the tab bar's close box, TAB-021).
fn close_tab(hwnd: HWND, idx: usize) {
    if state_mut(hwnd).is_some_and(|s| s.app.close_tab(idx)) {
        let _ = unsafe { DestroyWindow(hwnd) };
        return;
    }
    sync_tab_bar(hwnd);
    sync_active_tab_geometry(hwnd); // issue #9: the successor tab may predate a resize
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// CONFIG-010: copy the current selection, when there is one.
fn copy_selection(hwnd: HWND) {
    let text = state_ref(hwnd).and_then(|s| {
        let tab = s.app.active_tab()?;
        tab.pane().selection_text()
    });
    if let Some(text) = text
        && !text.is_empty()
    {
        let _ = clipboard::set_text(&text);
    }
}

/// `[keybindings]` spec for a key event: "ctrl+shift+t" (CONFIG-008).
fn key_spec(vk: u16, mods: Modifiers) -> String {
    let mut spec = String::new();
    if mods.ctrl {
        spec.push_str("ctrl+");
    }
    if mods.alt {
        spec.push_str("alt+");
    }
    if mods.shift {
        spec.push_str("shift+");
    }
    spec.push_str(&key_name(vk));
    spec
}

/// Key names accepted in `[keybindings]` specs.
fn key_name(vk: u16) -> String {
    let named = match vk {
        0x08 => "backspace",
        0x09 => "tab",
        0x0D => "enter",
        0x1B => "escape",
        0x20 => "space",
        0x21 => "pageup",
        0x22 => "pagedown",
        0x23 => "end",
        0x24 => "home",
        0x25 => "left",
        0x26 => "up",
        0x27 => "right",
        0x28 => "down",
        0x2D => "insert",
        0x2E => "delete",
        0x6D => "minus",
        0xBB => "equal",
        0xDB => "left-bracket",
        0xDD => "right-bracket",
        0xDC => "backslash",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    if (0x41..=0x5A).contains(&vk) {
        return ((vk as u8 + 32) as char).to_string();
    }
    if (0x30..=0x39).contains(&vk) {
        return char::from(u8::try_from(vk).unwrap_or(b'0') - 0x30 + b'0').to_string();
    }
    if (0x70..=0x87).contains(&vk) {
        return format!("f{}", vk - 0x70 + 1);
    }
    String::new()
}

/// Run a `[keybindings]` action; every action maps to a menu command.
fn run_key_action(hwnd: HWND, action: KeyAction) {
    match action {
        KeyAction::NewTab => new_tab(hwnd),
        KeyAction::CloseTab => close_pane_or_tab(hwnd),
        KeyAction::NextTab => switch_tab_rel(hwnd, 1),
        KeyAction::PrevTab => switch_tab_rel(hwnd, -1),
        KeyAction::MoveTabLeft => move_active_tab(hwnd, -1),
        KeyAction::MoveTabRight => move_active_tab(hwnd, 1),
        KeyAction::RenameTab => start_rename_tab(hwnd),
        KeyAction::Copy => copy_screen(hwnd),
        KeyAction::Paste => paste_clipboard(hwnd),
        KeyAction::Search => toggle_search(hwnd),
        KeyAction::SplitRight => split_pane(hwnd, Dir::Vertical),
        KeyAction::SplitDown => split_pane(hwnd, Dir::Horizontal),
        KeyAction::Fullscreen => toggle_fullscreen(hwnd),
        KeyAction::Quit => {
            let _ = unsafe { DestroyWindow(hwnd) };
        }
        KeyAction::FocusUp => focus_side(hwnd, Side::Up),
        KeyAction::FocusDown => focus_side(hwnd, Side::Down),
        KeyAction::FocusLeft => focus_side(hwnd, Side::Left),
        KeyAction::FocusRight => focus_side(hwnd, Side::Right),
        KeyAction::ResizeUp => resize_side(hwnd, Side::Up),
        KeyAction::ResizeDown => resize_side(hwnd, Side::Down),
        KeyAction::ResizeLeft => resize_side(hwnd, Side::Left),
        KeyAction::ResizeRight => resize_side(hwnd, Side::Right),
        KeyAction::PrevTheme => cycle_theme(hwnd, -1),
        KeyAction::NextTheme => cycle_theme(hwnd, 1),
        KeyAction::PrevProfile => cycle_profile(hwnd, -1),
        KeyAction::NextProfile => cycle_profile(hwnd, 1),
        KeyAction::CloseQuick => {
            // Only the quick window answers this; it hides, never closes.
            if state_ref(hwnd).is_some_and(|s| s.quick) {
                let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            }
        }
        KeyAction::IncreaseFontSize => zoom_font(hwnd, Some(1.0)),
        KeyAction::DecreaseFontSize => zoom_font(hwnd, Some(-1.0)),
        KeyAction::ResetFontSize => zoom_font(hwnd, None),
        KeyAction::ReloadConfig => {
            if let Some(s) = state_mut(hwnd) {
                reload_config(s, hwnd);
            }
        }
        KeyAction::ClearScreen => {
            // TTY-016: shells that support Ctrl-L clear their own screen.
            if let Some(s) = state_mut(hwnd) {
                let _ = s.app.send_input_active(b"\x0c");
            }
        }
        KeyAction::PrevCommand => command_jump(hwnd, -1),
        KeyAction::NextCommand => command_jump(hwnd, 1),
        KeyAction::CopyLastCommand => copy_last_command(hwnd),
        KeyAction::CommandPalette => toggle_palette(hwnd),
    }
}

/// SHELLINT-022 (issue #38): Ctrl+Shift+Up/Down jumps the focused pane's
/// scroll to the previous/next command boundary. The anchor is the top
/// visible line, so repeated presses walk the history like a jumplist.
fn command_jump(hwnd: HWND, delta: i32) {
    let Some(s) = state_mut(hwnd) else { return };
    let Some(tab) = s.app.active_tab_mut() else {
        return;
    };
    let mut pane = tab.pane_mut();
    let anchor = pane
        .grid()
        .abs_of_view(0)
        .unwrap_or_else(|| pane.grid().last_abs());
    let target = if delta < 0 {
        pane.commands().prev_boundary(anchor)
    } else {
        pane.commands().next_boundary(anchor)
    };
    if let Some(abs) = target {
        pane.grid_mut().scroll_to_abs(abs);
        pane.update_follow();
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    }
}

/// SHELLINT-023 (issue #38): copy the last finished command's output block
/// to the clipboard.
fn copy_last_command(hwnd: HWND) {
    let Some(s) = state_ref(hwnd) else { return };
    let Some(tab) = s.app.active_tab() else {
        return;
    };
    let pane = tab.pane();
    let Some(text) = pane
        .commands()
        .last_finished()
        .and_then(|c| pane.command_output_text(c.id))
    else {
        return;
    };
    if !clipboard::set_text(&text) {
        eprintln!("rustty: clipboard copy failed");
    }
}

/// Pane focus by keyboard (SPLIT-004).
fn focus_side(hwnd: HWND, side: Side) {
    if state_mut(hwnd).is_some_and(|s| s.app.focus_neighbor(side)) {
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    }
}

/// Pane resize by keyboard (SPLIT-005).
fn resize_side(hwnd: HWND, side: Side) {
    let resized = state_mut(hwnd).is_some_and(|s| {
        let step = App::ratio_step();
        let (dir, delta) = match side {
            Side::Left => (Dir::Vertical, -step),
            Side::Right => (Dir::Vertical, step),
            Side::Up => (Dir::Horizontal, -step),
            Side::Down => (Dir::Horizontal, step),
        };
        s.app.adjust_ratio(dir, delta)
    });
    if resized {
        sync_tab_bar(hwnd);
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    }
}

/// THEME-010: cycle the built-in themes.
fn cycle_theme(hwnd: HWND, delta: i32) {
    let Some(s) = state_mut(hwnd) else {
        return;
    };
    let n = theme::PRESETS.len() as i32;
    let idx = (active_theme_index(s) as i32 + delta).rem_euclid(n) as usize;
    s.theme = Theme::preset(theme::PRESETS[idx]).unwrap_or_default();
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

/// SHELL-022: cycle profiles; the selector moves and so does the active tab.
fn cycle_profile(hwnd: HWND, delta: i32) {
    let Some(s) = state_mut(hwnd) else {
        return;
    };
    let n = s.profiles.len() as i32;
    let idx = (s.profile as i32 + delta).rem_euclid(n) as usize;
    s.profile = idx;
    let spec = s.profiles[idx].clone();
    let tab = s.app.active_index();
    if let Some(tab) = s.app.tab_at_mut(tab) {
        tab.set_profile(spec.name.clone());
        tab.set_auto_title(spec.name.clone());
        tab.set_color(spec.color);
    }
    apply_profile_theme(s, &spec);
    apply_profile_font(s, &spec);
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
        eprintln!("rustty: clipboard copy failed");
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

/// Scrollback lines the UIA grid value exposes above the viewport
/// (UIA-002: "recent scrollback text", best-effort cap).
const UIA_SCROLLBACK_CONTEXT_LINES: usize = 500;

/// UIA-001..005 (WIN-007): rebuild the snapshot the provider serves, from
/// the live window state. Built fresh on every call — there is no cache to
/// keep in sync with the frame. All rects are client DIPs, laid out
/// exactly as the renderer paints them (the tab bar sits above the search
/// strip when the strip is up, SEARCH-001).
pub(crate) fn uia_snapshot(hwnd: HWND) -> Option<crate::uia::Snapshot> {
    let state = state_ref(hwnd)?;
    let renderer = state.renderer.as_ref()?;
    let (client_w, client_h) = renderer.client_dips();
    let m = renderer.metrics();
    let bar = state.bar_height();
    // PALETTE-001: the palette overlay takes the strip from search, and is
    // as tall as its rows. Only one overlay is open at a time (window
    // layer keeps them exclusive).
    let palette_rows = state
        .palette
        .as_ref()
        .map_or(0, |p| p.hits().len().min(crate::palette::VISIBLE_ROWS));
    let strip = if state.palette.is_some() {
        let row_h = palette_row_height(m.cell_height);
        (row_h * (1 + palette_rows) as f32).min(client_h.max(0.0))
    } else if state.search.is_some() || state.renaming.is_some() {
        SEARCH_BAR_HEIGHT.min(client_h.max(0.0))
    } else {
        0.0
    };
    let bar_top = if state.cfg.tab_bar.placement == TabBarPlacement::Bottom {
        (client_h - bar - strip).max(0.0)
    } else {
        0.0
    };
    let mut snap = crate::uia::Snapshot {
        title: String::new(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        tabs: Vec::new(),
        active: state.app.active_index(),
        bar_visible: bar > 0.0,
        bar_bounds: (0.0, bar_top, client_w, bar_top + bar),
        tab_bounds: Vec::new(),
        client_w,
        client_h,
        dpi: renderer.dpi(),
        grid_bounds: Vec::new(),
        panes: Vec::new(),
        focused_pane: 0,
        search_bounds: None,
        search_query: None,
        search_matches: None,
        palette_bounds: None,
        palette_query: None,
        palette_matches: None,
    };
    let mut caption = [0u16; 512];
    let n = usize::try_from(unsafe { GetWindowTextW(hwnd, &mut caption) }).unwrap_or(0);
    snap.title = String::from_utf16_lossy(&caption[..n.min(caption.len())]);
    for tab in state.app.tabs() {
        let focused = tab.focused_index();
        let mut idx = 0usize;
        let mut cwd = None;
        let mut host = None;
        tab.for_each_pane_ref(&mut |pane: &Pane, _| {
            if Some(idx) == focused {
                cwd = pane.cwd().map(str::to_string);
                host = pane.shell_host().map(str::to_string);
            }
            idx += 1;
        });
        snap.tabs.push(crate::uia::TabInfo {
            title: tab.title().to_string(),
            profile: tab.profile().to_string(),
            cwd,
            host,
            activity: tab.bell_pending(),
        });
    }
    snap.tab_bounds = renderer.tab_layout().tabs;
    if let Some(tab) = state.app.active_tab() {
        snap.focused_pane = tab.focused_index().unwrap_or(0);
        tab.for_each_pane_ref(&mut |pane: &Pane, rect: Rect| {
            let grid = pane.grid();
            let rows = grid.rows();
            let viewport_top = grid.total_lines().saturating_sub(rows);
            let mut screen_text = String::new();
            for r in 0..rows {
                if r > 0 {
                    screen_text.push('\n');
                }
                if let Some(line) = grid.line_text(r) {
                    screen_text.push_str(&line);
                }
            }
            let start = viewport_top.saturating_sub(UIA_SCROLLBACK_CONTEXT_LINES);
            let mut scrollback_text = String::new();
            for l in start..viewport_top {
                if !scrollback_text.is_empty() {
                    scrollback_text.push('\n');
                }
                if let Some(line) = grid.line_text_abs(l) {
                    scrollback_text.push_str(&line);
                }
            }
            let cursor = if grid.scroll_offset() == 0 {
                Some((pane.parser().cursor_row, pane.parser().cursor_col))
            } else {
                None
            };
            snap.grid_bounds.push((
                renderer.grid_left() + rect.col as f32 * m.cell_width,
                renderer.grid_top() + rect.row as f32 * m.cell_height,
                renderer.grid_left() + (rect.col + rect.cols) as f32 * m.cell_width,
                renderer.grid_top() + (rect.row + rect.rows) as f32 * m.cell_height,
            ));
            snap.panes.push(crate::uia::PaneInfo {
                screen_text,
                scrollback_text,
                cursor,
                scrollback_lines: viewport_top,
                selection: pane.selection_text(),
            });
        });
    }
    if let Some(s) = &state.search {
        snap.search_bounds = Some((0.0, (client_h - strip).max(0.0), client_w, client_h));
        snap.search_query = Some(s.query.clone());
        snap.search_matches = Some((s.current, s.matches.len()));
    }
    if let Some(p) = &state.palette {
        snap.palette_bounds = Some((0.0, (client_h - strip).max(0.0), client_w, client_h));
        snap.palette_query = Some(p.query.clone());
        snap.palette_matches = Some((p.current, p.hits().len()));
    }
    Some(snap)
}

/// UIA-002: the window rect in physical screen pixels plus the client
/// origin, the pair that turns the snapshot's client DIPs into screen px.
pub(crate) fn uia_geometry(hwnd: HWND) -> Option<(DipRect, (f32, f32))> {
    state_ref(hwnd)?;
    let mut win = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut win) }.is_err() {
        return None;
    }
    let mut origin = POINT { x: 0, y: 0 };
    if !unsafe { ClientToScreen(hwnd, &mut origin) }.as_bool() {
        return None;
    }
    Some((
        (
            win.left as f32,
            win.top as f32,
            win.right as f32,
            win.bottom as f32,
        ),
        (origin.x as f32, origin.y as f32),
    ))
}

/// Rebuild the taskbar Jump List (one `--profile` task per profile plus
/// workspace tasks). Shared by startup and the discovery WM_APP (issue #40).
fn rebuild_jumplist(names: &[String]) {
    match std::env::current_exe() {
        Ok(exe) => {
            let tasks = crate::jumplist::tasks_for(names, &crate::workspace::names());
            let res =
                crate::jumplist::build(&aumid().to_string(), &exe.display().to_string(), &tasks);
            // shortcut: Windows rejects user tasks for unpackaged exes with
            // no Start Menu shortcut (E_INVALIDARG), so every cargo dev run
            // would print this. Drop the guard when packaging installs one.
            // (window.rs is Windows-only, so the `\target\` separator is fine.)
            let dev_build = exe.to_string_lossy().contains(r"\target\");
            if let Err(e) = res
                && !dev_build
            {
                eprintln!("rustty: jump list failed: {e}");
            }
        }
        Err(e) => eprintln!("rustty: jump list skipped: {e}"),
    }
}

/// Open the terminal window and run the message loop until the last tab
/// closes (spec WIN-001: this is the UI thread that owns ConPTY).
/// `workspace` (WORKSPACE-004): open this template instead of reading
/// `session.toml`.
///
/// # Errors
///
/// Fails when the window class cannot be registered or the window cannot
/// be created.
pub fn run(
    cfg: &Config,
    theme: Theme,
    spawn: SpawnOptions,
    workspace: Option<&str>,
) -> anyhow::Result<()> {
    // Per-monitor DPI v2 before any window metrics exist (WIN-002).
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    // Roadmap v0.2 "Windows Jump List": taskbar right-click gets a plain
    // "New window" task plus one `--profile` task per profile. The process
    // AUMID matches the toasts (PKG-002) so Windows attributes both.
    let _ = unsafe { SetCurrentProcessExplicitAppUserModelID(&aumid()) };
    let names: Vec<String> = merge_profiles(cfg, &discovered_now())
        .into_iter()
        .map(|p| p.name)
        .collect();
    rebuild_jumplist(&names);
    let dpi = unsafe {
        let d = GetDpiForSystem();
        if d == 0 { 96 } else { d }
    };
    let font = cfg.font();
    let metrics = measure_cells(dpi, &font)?;
    let cols = cfg.window.cols.max(MIN_COLS);
    let rows = cfg.window.rows.max(MIN_ROWS);
    // One tab at startup: "auto"/"hidden" bars start collapsed (TAB-010).
    let bar = match cfg.tab_bar.visible {
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

    // TAB-026/WINDOW-015/016: the saved session restores tabs at
    // `init_window` and the window frame here. A saved position only
    // applies when its top-left point is on a live monitor (a
    // disconnected screen must not park the window off-screen).
    // WORKSPACE-004: `--workspace <name>` replaces the session file
    // entirely: the template defines the tabs, the window frame is the
    // configured default.
    let (session, ws_source, ws_pending) = match workspace {
        Some(name) => {
            let found = crate::workspace::store_path()
                .and_then(|p| crate::workspace::workspace_at(&p, name));
            match found {
                Some(ws) => {
                    let pending = ws_leaf_cmds(&ws);
                    (Some(ws.as_session()), Some(ws.name), Some(pending))
                }
                None => (None, None, None),
            }
        }
        None if cfg.session_restore => (crate::session::load(), None, None),
        None => (None, None, None),
    };
    let (x, y, width, height) = session
        .as_ref()
        .and_then(|se| se.window)
        .filter(|wr| wr.w >= 200 && wr.h >= 100)
        .filter(|wr| {
            !unsafe { MonitorFromPoint(POINT { x: wr.x, y: wr.y }, MONITOR_DEFAULTTONULL) }
                .is_invalid()
        })
        .map_or(
            (
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                rect.right - rect.left,
                rect.bottom - rect.top,
            ),
            |wr| (wr.x, wr.y, wr.w, wr.h),
        );

    // Roadmap v0.2: recipe for the quick window, used to (re)create it when
    // its shell died.
    if cfg.quick_terminal.enabled
        && let Ok(mut spec) = QUICK_SPEC.lock()
    {
        *spec = Some((cfg.clone(), theme.clone(), spawn.clone()));
    }

    // WORKSPACE-007: `--workspace <name>` with no matching template —
    // `ws_source` moves into the state below, so record the miss here.
    let ws_miss = workspace.filter(|_| ws_source.is_none());

    let state = Box::new(WindowState {
        restore: session,
        ws_source,
        ws_pending,
        cmd_notified: 0,
        drag_resize: false,
        app: {
            let mut app = App::with_size(cols, rows, cfg.scrollback);
            // TAB-025: tab titles fall back to the pane's OSC 7 cwd.
            app.set_show_cwd(cfg.tab_bar.show_cwd);
            app
        },
        theme,
        spawn,
        font: font.clone(),
        tab_bar: cfg.tab_bar.visible,
        bell_tabs: Vec::new(),
        bell_flash: false,
        cfg_mtime: None,
        quick: false,
        profiles: merge_profiles(cfg, &discovered_now()),
        cfg: cfg.clone(),
        renaming: None,
        tab_drag: None,
        blink_on: true,
        theme_dir: theme_dir_from_config(),
        renderer: None,
        metrics,
        timer_id: 0,
        drag_anchor: None,
        last_click: None,
        link_target: None,
        suppress_char: false,
        pending_surrogate: None,
        fullscreen: None,
        search: None,
        palette: None,
        recent: Vec::new(),
        // CONFIG-013: `starting-profile` picks the initial shell.
        profile: {
            let merged = merge_profiles(cfg, &discovered_now());
            merged
                .iter()
                .position(|p| p.name.eq_ignore_ascii_case(&cfg.starting_profile))
                .unwrap_or(0)
        },
    });
    let state_param = Box::into_raw(state) as *const c_void;

    let hinstance = unsafe { GetModuleHandleW(None) }?;
    // The app icon build.rs embeds (winresource puts its icon group at
    // resource ID 1 = MAKEINTRESOURCE(1)). A NULL class hIcon leaves the
    // title bar showing the generic window glyph.
    #[allow(
        clippy::manual_dangling_ptr,
        reason = "MAKEINTRESOURCE(1): Win32 passes resource IDs as pseudo-pointers"
    )]
    let icon = unsafe {
        LoadIconW(
            Some(hinstance.into()),
            PCWSTR::from_raw(1usize as *const u16),
        )
    }
    .ok()
    .unwrap_or_default();
    let class_name = wstr(CLASS_NAME);
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }?;
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance.into(),
        hIcon: icon,
        hCursor: cursor,
        hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(anyhow::anyhow!("rustty: window class registration failed"));
    }

    let title = wstr(APP_NAME);
    let created = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            style,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(hinstance.into()),
            Some(state_param),
        )
    };
    if let Err(e) = created {
        // The window never existed to reclaim the box; free it here.
        unsafe { drop(Box::from_raw(state_param as *mut WindowState)) };
        return Err(anyhow::anyhow!("rustty: window creation failed: {e}"));
    }
    if let Some(name) = ws_miss {
        // WORKSPACE-007: a missing template opens a plain window and says why.
        let msg = wstr(&format!(
            "rustty: no workspace template named {name:?}.\n\nA plain window opened instead."
        ));
        let caption = wstr("rustty");
        if let Ok(main) = created {
            unsafe {
                let _ = MessageBoxW(
                    Some(main),
                    PCWSTR(msg.as_ptr()),
                    PCWSTR(caption.as_ptr()),
                    MB_OK | MB_ICONWARNING,
                );
            }
        }
    }

    // Roadmap v0.2 "Hotkey summon from screen edge": the quick terminal is
    // a hidden drop-down window; the global hotkey lives on the main window
    // so WM_HOTKEY arrives on the message loop that owns every shell.
    if cfg.quick_terminal.enabled {
        create_quick_window();
        if let Ok(main) = created
            && register_hotkey(main, &cfg.quick_terminal).is_err()
        {
            eprintln!(
                "rustty: quick-terminal hotkey `{}` is taken by another app; the quick terminal stays off",
                cfg.quick_terminal.key
            );
        }
    }

    // Issue #40: run the WSL/VS/PowerShell discovery probes on a worker
    // thread (startup never waits, DISCOVERY-006); WM_APP re-merges the
    // profile list when fresh results land.
    if let Ok(main) = created {
        start_discovery(main);
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
    fn toast_text_is_escaped() {
        assert_eq!(
            xml_escape("make && rm -rf \"x\" <y> 'z'"),
            "make &amp;&amp; rm -rf &quot;x&quot; &lt;y&gt; &apos;z&apos;"
        );
    }

    #[test]
    fn menu_generated_command_ranges_stay_disjoint() {
        assert_eq!(theme_command(MENU_THEME_BASE + 1), Some(1));
        assert_eq!(theme_command(MENU_THEME_BASE - 1), None);
    }

    #[test]
    fn hotkey_names_map_to_virtual_keys() {
        assert_eq!(hotkey_vk("grave"), Some(0xC0));
        assert_eq!(hotkey_vk("j"), Some(0x4A));
        assert_eq!(hotkey_vk("f2"), Some(0x71));
        assert_eq!(hotkey_vk("plus"), Some(0xBB));
        assert_eq!(hotkey_vk("banana"), None);
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn tab_colour_hex_accepts_hash_and_bare_forms() {
        assert_eq!(Color::parse_hex("#ff0000"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(Color::parse_hex("00ff00"), Some(Color::rgb(0, 255, 0)));
        assert_eq!(Color::parse_hex("nope"), None);
    }

    #[test]
    fn a_config_profile_overrides_the_builtin_with_the_same_name() {
        let cfg = Config::parse("[[profile]]\nname = \"PowerShell\"\ncommand = \"pwsh-preview\"\n")
            .expect("parses");
        let merged = merge_profiles(&cfg, &[]);
        let ps = merged
            .iter()
            .find(|p| p.name == "PowerShell")
            .expect("present");
        assert_eq!(ps.program, "pwsh-preview");
        assert_eq!(merged.iter().filter(|p| p.name == "PowerShell").count(), 1);
    }

    #[test]
    fn key_names_match_the_keybinding_specs() {
        assert_eq!(key_name(0x54), "t");
        assert_eq!(key_name(0x21), "pageup");
        assert_eq!(key_name(0x70), "f1");
        let mods = Modifiers {
            ctrl: true,
            shift: true,
            alt: false,
        };
        assert_eq!(key_spec(0x21, mods), "ctrl+shift+pageup");
    }
}
