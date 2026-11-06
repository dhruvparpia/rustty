//! Shortcuts pressed as keys, not chosen from a menu. `Ctrl+Shift+Enter` once
//! matched a key code Windows never sends, so it never split anything, and tests
//! that went through the menu could not see that. This test shares a keyboard state
//! with the window alone, so the person's own keyboard focus is left alone.
#![cfg(windows)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
#![allow(
    unsafe_code,
    reason = "test driver: Win32 keyboard state for one window"
)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardState, SetKeyboardState};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetWindowThreadProcessId, PostMessageW, WM_CHAR, WM_KEYDOWN, WM_KEYUP,
};
use windows::core::{PCWSTR, w};

const VK_RETURN: usize = 0x0D;
const VK_MINUS: usize = 0xBD;
const VK_SHIFT: usize = 0x10;
const VK_CONTROL: usize = 0x11;
const VK_LCONTROL: usize = 0xA2;

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
