//! Shortcuts pressed as keys, not chosen from a menu. `Ctrl+Shift+Enter` once
//! matched a key code Windows never sends, so it never split anything, and tests
//! that went through the menu could not see that. This test shares a keyboard state
//! with the window alone, so the person's own keyboard focus is left alone.
//!
//! Panes dragged by their title rows are here too, posted as mouse messages to the
//! window, so the person's pointer is left alone as well.
#![cfg(windows)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
#![allow(
    unsafe_code,
    reason = "test driver: Win32 keyboard state for one window"
)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardState, SetKeyboardState};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetClientRect, GetWindowThreadProcessId, PostMessageW, WM_CHAR, WM_KEYDOWN,
    WM_KEYUP,
};
use windows::core::{PCWSTR, w};

const VK_RETURN: usize = 0x0D;
const VK_MINUS: usize = 0xBD;
const VK_SHIFT: usize = 0x10;
const VK_CONTROL: usize = 0x11;
const VK_LCONTROL: usize = 0xA2;
const VK_ESCAPE: usize = 0x1B;
const WM_MOUSEMOVE: u32 = 0x200;
const WM_LBUTTONDOWN: u32 = 0x201;
const WM_LBUTTONUP: u32 = 0x202;
/// Inside a title row: the grid starts about 47 DIPs down and a row is about 21.
const TITLE_Y: f32 = 57.0;
/// Inside the sidebar's right edge, which is about 256 DIPs wide.
const RAIL_W: f32 = 256.0;

struct Window {
    child: Child,
    scratch: PathBuf,
    hwnd: HWND,
}

impl Window {
    fn open(name: &str, args: &[&str], prepare: impl FnOnce(&Path)) -> Self {
        let scratch =
            std::env::temp_dir().join(format!("rustty-keys-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let settings = scratch.join("rustty");
        std::fs::create_dir_all(&settings).expect("scratch settings");
        std::fs::write(settings.join("config.toml"), "automation-reads = true\n").expect("config");
        prepare(&settings);
        let child = Command::new(env!("CARGO_BIN_EXE_rustty"))
            .args(args)
            .env("APPDATA", &scratch)
            .env("LOCALAPPDATA", &scratch)
            .current_dir(&scratch)
            .spawn()
            .expect("rustty starts");
        let hwnd = find_window(child.id());
        let window = Self {
            child,
            scratch,
            hwnd,
        };
        window.wait_for("a first pane", |list| panes(list) >= 1);
        // The shell has to be up before keys mean anything.
        std::thread::sleep(Duration::from_secs(2));
        window
    }

    fn list(&self) -> serde_json::Value {
        let out = Command::new(env!("CARGO_BIN_EXE_rustty"))
            .args(["ctl", "list", "--json"])
            .env("APPDATA", &self.scratch)
            .env("LOCALAPPDATA", &self.scratch)
            .env_remove("RUSTTY_WINDOW")
            .output()
            .expect("ctl runs");
        serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null)
    }

    fn wait_for(&self, what: &str, done: impl Fn(&serde_json::Value) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last = serde_json::Value::Null;
        while Instant::now() < deadline {
            last = self.list();
            if done(&last) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("never saw {what}: {last}");
    }

    /// Post a mouse message at client position `(x, y)` in DIPs.
    fn mouse(&self, message: u32, held: bool, x: f32, y: f32) {
        // SAFETY: plain window queries and a posted message; a stale handle just fails.
        unsafe {
            let scale = GetDpiForWindow(self.hwnd) as f32 / 96.0;
            let (x, y) = ((x * scale) as isize, (y * scale) as isize);
            PostMessageW(
                Some(self.hwnd),
                message,
                WPARAM(usize::from(held)),
                LPARAM((y << 16) | x),
            )
            .expect("post");
        }
        std::thread::sleep(Duration::from_millis(150));
    }

    /// The middle of the left and right panes of a side-by-side split, in DIPs.
    fn pane_columns(&self) -> (f32, f32) {
        let mut client = RECT::default();
        // SAFETY: `client` is a valid out-pointer; a stale handle just fails.
        let scale = unsafe {
            GetClientRect(self.hwnd, &raw mut client).expect("client rect");
            GetDpiForWindow(self.hwnd) as f32 / 96.0
        };
        let width = client.right as f32 / scale - RAIL_W;
        (RAIL_W + width * 0.25, RAIL_W + width * 0.75)
    }

    /// Split side by side, and return the pane ids left to right.
    fn side_by_side(&self) -> (u64, u64) {
        self.ctrl_shift(VK_RETURN, Some('\n'));
        self.wait_for("two panes", |list| panes(list) == 2);
        let ids = pane_ids(&self.list());
        (ids[0], ids[1])
    }

    /// Press a key with Ctrl and Shift held, as far as this window can tell.
    fn ctrl_shift(&self, vk: usize, typed: Option<char>) {
        // SAFETY: the keyboard state is a 256-byte array and is restored below;
        // the threads are detached again before returning.
        unsafe {
            let mut pid = 0;
            let thread = GetWindowThreadProcessId(self.hwnd, Some(&raw mut pid));
            let me = GetCurrentThreadId();
            assert!(AttachThreadInput(me, thread, true).as_bool(), "attach");
            let mut keys = [0u8; 256];
            GetKeyboardState(&mut keys).expect("read the keyboard state");
            let before = keys;
            for held in [VK_CONTROL, VK_LCONTROL, VK_SHIFT] {
                keys[held] = 0x80;
            }
            SetKeyboardState(&keys).expect("set the keyboard state");
            PostMessageW(Some(self.hwnd), WM_KEYDOWN, WPARAM(vk), LPARAM(0)).expect("post");
            if let Some(c) = typed {
                PostMessageW(Some(self.hwnd), WM_CHAR, WPARAM(c as usize), LPARAM(0))
                    .expect("post");
            }
            PostMessageW(Some(self.hwnd), WM_KEYUP, WPARAM(vk), LPARAM(0)).expect("post");
            std::thread::sleep(Duration::from_millis(600));
            SetKeyboardState(&before).expect("restore the keyboard state");
            let _ = AttachThreadInput(me, thread, false);
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // Only the process this test started, and what it started.
        let _ = Command::new("taskkill")
            .args(["/PID", &self.child.id().to_string(), "/T", "/F"])
            .output();
        let _ = self.child.wait();
        std::thread::sleep(Duration::from_millis(200));
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

fn find_window(pid: u32) -> HWND {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let mut at = None;
        // SAFETY: plain window queries; a stale handle just fails.
        unsafe {
            while let Ok(found) = FindWindowExW(None, at, w!("rustty"), PCWSTR::null()) {
                let mut owner = 0;
                GetWindowThreadProcessId(found, Some(&raw mut owner));
                if owner == pid {
                    return found;
                }
                at = Some(found);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("no window");
}

fn pane_ids(list: &serde_json::Value) -> Vec<u64> {
    list["tabs"][0]["panes"]
        .as_array()
        .map_or_else(Vec::new, |panes| {
            panes.iter().filter_map(|p| p["id"].as_u64()).collect()
        })
}

fn active_pane(list: &serde_json::Value) -> Option<u64> {
    list["tabs"][0]["panes"]
        .as_array()?
        .iter()
        .find(|p| p["active"] == true)?["id"]
        .as_u64()
}

fn panes(list: &serde_json::Value) -> usize {
    list["tabs"][0]["panes"].as_array().map_or(0, Vec::len)
}

#[test]
#[ignore = "needs a desktop session: cargo test --test keyboard_live -- --ignored"]
fn ctrl_shift_enter_splits_right() {
    let window = Window::open("right", &[], |_| {});
    // Windows sends a line feed for Ctrl+Enter once the key is translated.
    window.ctrl_shift(VK_RETURN, Some('\n'));
    window.wait_for("two panes", |list| panes(list) == 2);
}

#[test]
#[ignore = "needs a desktop session: cargo test --test keyboard_live -- --ignored"]
fn ctrl_shift_minus_splits_below() {
    let window = Window::open("below", &[], |_| {});
    window.ctrl_shift(VK_MINUS, None);
    window.wait_for("two panes", |list| panes(list) == 2);
}

#[test]
#[ignore = "needs a desktop session: cargo test --test keyboard_live -- --ignored"]
fn dragging_a_title_onto_the_middle_of_a_pane_swaps_them() {
    let window = Window::open("swap", &[], |_| {});
    let (left, right) = window.side_by_side();
    let (left_x, right_x) = window.pane_columns();
    window.mouse(WM_LBUTTONDOWN, true, right_x, TITLE_Y);
    window.mouse(WM_MOUSEMOVE, true, left_x, 250.0);
    window.mouse(WM_LBUTTONUP, false, left_x, 250.0);
    window.wait_for("the panes swapped", |list| pane_ids(list) == [right, left]);
    assert_eq!(
        active_pane(&window.list()),
        Some(right),
        "it keeps the keyboard"
    );
}

#[test]
#[ignore = "needs a desktop session: cargo test --test keyboard_live -- --ignored"]
fn dragging_a_title_onto_the_edge_of_a_pane_puts_it_there() {
    let window = Window::open("edge", &[], |_| {});
    let (left, right) = window.side_by_side();
    let (left_x, right_x) = window.pane_columns();
    // Just below the left pane's title row: the top edge of that pane.
    window.mouse(WM_LBUTTONDOWN, true, right_x, TITLE_Y);
    window.mouse(WM_MOUSEMOVE, true, left_x, TITLE_Y + 25.0);
    window.mouse(WM_LBUTTONUP, false, left_x, TITLE_Y + 25.0);
    window.wait_for("the pane above the other", |list| {
        pane_ids(list) == [right, left]
    });
    // Stacked, not swapped: the moved pane now spans the top, so its title is also
    // above the old right column. After a swap that title would be the other pane's.
    window.mouse(WM_LBUTTONDOWN, true, right_x, TITLE_Y);
    window.mouse(WM_LBUTTONUP, false, right_x, TITLE_Y);
    window.wait_for("the moved pane across the whole top", |list| {
        active_pane(list) == Some(right)
    });
}

#[test]
#[ignore = "needs a desktop session: cargo test --test keyboard_live -- --ignored"]
fn clicking_a_title_focuses_the_pane_and_moves_nothing() {
    let window = Window::open("click", &[], |_| {});
    let (left, right) = window.side_by_side();
    let (left_x, _) = window.pane_columns();
    assert_eq!(active_pane(&window.list()), Some(right));
    window.mouse(WM_LBUTTONDOWN, true, left_x, TITLE_Y);
    window.mouse(WM_LBUTTONUP, false, left_x, TITLE_Y);
    window.wait_for("the left pane focused", |list| {
        active_pane(list) == Some(left)
    });
    assert_eq!(pane_ids(&window.list()), [left, right]);
}

#[test]
#[ignore = "needs a desktop session: cargo test --test keyboard_live -- --ignored"]
fn escape_puts_a_dragged_pane_back() {
    let window = Window::open("escape", &[], |_| {});
    let (left, right) = window.side_by_side();
    let (left_x, right_x) = window.pane_columns();
    window.mouse(WM_LBUTTONDOWN, true, right_x, TITLE_Y);
    window.mouse(WM_MOUSEMOVE, true, left_x, 250.0);
    // SAFETY: a posted message; a stale handle just fails.
    unsafe {
        PostMessageW(Some(window.hwnd), WM_KEYDOWN, WPARAM(VK_ESCAPE), LPARAM(0)).expect("post");
    }
    std::thread::sleep(Duration::from_millis(200));
    window.mouse(WM_LBUTTONUP, false, left_x, 250.0);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(pane_ids(&window.list()), [left, right]);
}

#[test]
#[ignore = "needs a desktop session: cargo test --test keyboard_live -- --ignored"]
fn a_folder_on_the_command_line_opens_beside_the_restored_session() {
    // Break caught: `rustty C:\project` opening wherever the last run was, so King
    // Crab offered the wrong folder and the advice to pass one did not work.
    let wanted = std::env::temp_dir().join(format!("rustty-wanted-{}", std::process::id()));
    std::fs::create_dir_all(&wanted).expect("folder");
    let elsewhere = std::env::temp_dir().join(format!("rustty-other-{}", std::process::id()));
    std::fs::create_dir_all(&elsewhere).expect("folder");
    let saved = format!(
        "focused_tab = 0\n[[tabs]]\nprofile = \"PowerShell\"\ntitle = \"old\"\n[tabs.shape]\nkind = \"leaf\"\ncwd = \"{}\"\n",
        elsewhere.display().to_string().replace('\\', "/")
    );
    let window = Window::open("folder", &[&wanted.display().to_string()], |settings| {
        std::fs::write(settings.join("session.toml"), saved).expect("session");
    });
    let name = wanted
        .file_name()
        .expect("a name")
        .to_string_lossy()
        .into_owned();
    let active_is_wanted = |list: &serde_json::Value| {
        list["tabs"].as_array().is_some_and(|tabs| {
            tabs.iter().any(|tab| {
                tab["active"] == true
                    && tab["panes"][0]["cwd"]
                        .as_str()
                        .is_some_and(|cwd| cwd.ends_with(&name))
            })
        })
    };
    window.wait_for("the folder given, in front", active_is_wanted);
    // The saved layout is still there: passing a folder must not throw it away.
    window.wait_for("the restored tab too", |list| {
        list["tabs"].as_array().is_some_and(|tabs| tabs.len() == 2)
    });
    drop(window);
    let _ = std::fs::remove_dir_all(&wanted);
    let _ = std::fs::remove_dir_all(&elsewhere);
}
