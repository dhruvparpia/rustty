//! Skinned dialogs in a real window (DIALOG-001): the keyboard answers the right
//! button, and it paints in the theme's colors. Keys are posted to the focused
//! control, so they go through the same dialog message loop real keys do.
#![cfg(windows)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
#![allow(unsafe_code, reason = "DIALOG-001: test driver for a Win32 dialog")]

use std::sync::Mutex;
use std::time::{Duration, Instant};

use rustty::color::Color;
use rustty::dialog::{self, Answer, Palette, Spec, Tone};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GW_CHILD, GW_HWNDNEXT, GetClientRect, GetDlgCtrlID, GetWindow,
    GetWindowThreadProcessId, PostMessageW, SendMessageW, WM_CHAR, WM_COMMAND, WM_KEYDOWN,
    WM_KEYUP,
};
use windows::core::{PCWSTR, w};

/// One dialog on screen at a time: they are found by class.
static LOCK: Mutex<()> = Mutex::new(());

const VK_TAB: usize = 0x09;
const VK_RETURN: usize = 0x0D;
const VK_ESCAPE: usize = 0x1B;

fn colors() -> Palette {
    Palette {
        surface: Color::rgb(10, 20, 30),
        footer: Color::rgb(40, 50, 60),
        text: Color::rgb(240, 240, 240),
        border: Color::rgb(90, 90, 90),
        accent: Color::rgb(224, 164, 119),
        on_accent: Color::rgb(0, 0, 0),
        danger: Color::rgb(180, 40, 40),
        on_danger: Color::rgb(255, 255, 255),
        field: Color::rgb(5, 5, 5),
        dark: true,
    }
}

/// The dialog this process has open.
fn find_dialog() -> HWND {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let mut at = None;
        // SAFETY: plain window queries; a stale handle just fails.
        unsafe {
            while let Ok(found) = FindWindowExW(None, at, w!("#32770"), PCWSTR::null()) {
                let mut pid = 0;
                GetWindowThreadProcessId(found, Some(&raw mut pid));
                if pid == std::process::id() {
                    std::thread::sleep(Duration::from_millis(150));
                    return found;
                }
                at = Some(found);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("no dialog appeared");
}

/// The dialog's child with this control id.
fn child(dialog: HWND, id: i32) -> HWND {
    // SAFETY: walking this process's live windows.
    unsafe {
        let mut at = GetWindow(dialog, GW_CHILD).ok();
        while let Some(control) = at {
            if GetDlgCtrlID(control) == id {
                return control;
            }
            at = GetWindow(control, GW_HWNDNEXT).ok();
        }
    }
    panic!("no control {id}");
}

fn key(control: HWND, vk: usize) {
    // SAFETY: posting to a window of this process.
    unsafe {
        let _ = PostMessageW(Some(control), WM_KEYDOWN, WPARAM(vk), LPARAM(0));
        let _ = PostMessageW(Some(control), WM_KEYUP, WPARAM(vk), LPARAM(0));
    }
    std::thread::sleep(Duration::from_millis(80));
}

/// Open a dialog on its own thread, act on it, and return the answer.
fn with_dialog(spec: Spec<'static>, act: impl FnOnce(HWND)) -> Answer {
    let _guard = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let worker = std::thread::spawn(move || dialog::show(None, &colors(), &spec));
    act(find_dialog());
    worker.join().expect("the dialog thread ends")
}

fn question() -> Spec<'static> {
    Spec::question(
        Some("Delete it?"),
        "This can't be undone.",
        "&Delete",
        "&Cancel",
        Tone::Danger,
    )
}

#[test]
#[ignore = "needs a desktop session: cargo test --test dialog_live -- --ignored"]
fn enter_on_a_question_takes_the_safe_answer() {
    let answer = with_dialog(question(), |d| key(child(d, 7), VK_RETURN));
    assert!(!answer.confirmed);
}

#[test]
#[ignore = "needs a desktop session: cargo test --test dialog_live -- --ignored"]
fn tab_then_enter_takes_the_button_you_moved_to() {
    // Break caught: the drawn button can't be a default button, so Enter sent the
    // default's id, and Tab to Delete then Enter would have cancelled.
    let answer = with_dialog(question(), |d| {
        let no = child(d, 7);
        key(no, VK_TAB);
        key(child(d, 6), VK_RETURN);
    });
    assert!(answer.confirmed);
}

#[test]
#[ignore = "needs a desktop session: cargo test --test dialog_live -- --ignored"]
fn esc_cancels_from_any_button() {
    let answer = with_dialog(question(), |d| {
        key(child(d, 7), VK_TAB);
        key(child(d, 6), VK_ESCAPE);
    });
    assert!(!answer.confirmed);
}

#[test]
#[ignore = "needs a desktop session: cargo test --test dialog_live -- --ignored"]
fn a_buttons_letter_presses_it() {
    let answer = with_dialog(question(), |d| {
        let no = child(d, 7);
        // SAFETY: posting to a window of this process.
        let _ = unsafe { PostMessageW(Some(no), WM_CHAR, WPARAM('d' as usize), LPARAM(0)) };
        std::thread::sleep(Duration::from_millis(80));
    });
    assert!(answer.confirmed);
}

#[test]
#[ignore = "needs a desktop session: cargo test --test dialog_live -- --ignored"]
fn the_close_box_is_cancel() {
    let answer = with_dialog(question(), |d| {
        // SAFETY: the same message the close box sends.
        unsafe { SendMessageW(d, WM_COMMAND, Some(WPARAM(2)), Some(LPARAM(0))) };
    });
    assert!(!answer.confirmed);
}

#[test]
#[ignore = "needs a desktop session: cargo test --test dialog_live -- --ignored"]
fn a_program_can_choose_a_button() {
    // Automation and the test scripts send this.
    let answer = with_dialog(question(), |d| {
        // SAFETY: choosing a button on a dialog of this process.
        unsafe { SendMessageW(d, WM_COMMAND, Some(WPARAM(6)), Some(LPARAM(0))) };
    });
    assert!(answer.confirmed);
}

#[test]
#[ignore = "needs a desktop session: cargo test --test dialog_live -- --ignored"]
fn a_message_closes_on_enter() {
    let answer = with_dialog(Spec::message("Done."), |d| key(child(d, 1), VK_RETURN));
    assert!(answer.confirmed);
}

#[test]
#[ignore = "needs a desktop session: cargo test --test dialog_live -- --ignored"]
fn a_field_returns_what_was_typed() {
    let spec = Spec {
        title: "Name",
        heading: None,
        message: "Workspace name:",
        confirm: "OK",
        cancel: Some("Cancel"),
        tone: Tone::Normal,
        input: Some("start"),
        focus_confirm: true,
    };
    let answer = with_dialog(spec, |d| {
        let field = child(d, 1000);
        for c in "api".chars() {
            // SAFETY: posting to a window of this process.
            let _ = unsafe { PostMessageW(Some(field), WM_CHAR, WPARAM(c as usize), LPARAM(0)) };
        }
        std::thread::sleep(Duration::from_millis(80));
        key(field, VK_RETURN);
    });
    assert!(answer.confirmed);
    // The starting text was selected, so typing replaced it.
    assert_eq!(answer.text.as_deref(), Some("api"));
}

#[test]
#[ignore = "needs a desktop session: cargo test --test dialog_live -- --ignored"]
fn cancelling_a_field_returns_no_text() {
    let spec = Spec {
        title: "Name",
        heading: None,
        message: "Workspace name:",
        confirm: "OK",
        cancel: Some("Cancel"),
        tone: Tone::Normal,
        input: Some("start"),
        focus_confirm: true,
    };
    let answer = with_dialog(spec, |d| key(child(d, 1000), VK_ESCAPE));
    assert!(!answer.confirmed);
    assert_eq!(answer.text, None);
}

/// The dialog's client pixels, blue-green-red-unused.
fn capture(hwnd: HWND) -> (Vec<u8>, usize, usize) {
    let mut rect = RECT::default();
    // SAFETY: capturing a live window into a bitmap freed here.
    unsafe {
        GetClientRect(hwnd, &raw mut rect).expect("client rect");
        let (w, h) = (rect.right, rect.bottom);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(Some(hwnd));
        let mem = CreateCompatibleDC(Some(dc));
        let bitmap = CreateCompatibleBitmap(dc, w, h);
        let old = SelectObject(mem, HGDIOBJ::from(bitmap));
        // PW_CLIENTONLY.
        let _ = PrintWindow(hwnd, mem, PRINT_WINDOW_FLAGS(1));
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        GetDIBits(
            mem,
            bitmap,
            0,
            h as u32,
            Some(pixels.as_mut_ptr().cast()),
            &raw mut info,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, old);
        let _ = DeleteDC(mem);
        let _ = DeleteObject(HGDIOBJ::from(bitmap));
        ReleaseDC(Some(hwnd), dc);
        (pixels, w as usize, h as usize)
    }
}

#[test]
#[ignore = "needs a desktop session: cargo test --test dialog_live -- --ignored"]
fn it_paints_in_the_themes_colors() {
    // Break caught: a skin that only exists in the code.
    let answer = with_dialog(question(), |d| {
        let (pixels, w, h) = capture(d);
        let at = |x: usize, y: usize| {
            let i = (y * w + x) * 4;
            Color::rgb(pixels[i + 2], pixels[i + 1], pixels[i])
        };
        let p = colors();
        assert_eq!(at(2, 2), p.surface, "the body");
        assert_eq!(at(2, h - 2), p.footer, "the footer");
        let has = |want: Color| (0..w * h).any(|i| at(i % w, i / w) == want);
        assert!(has(p.danger), "a danger button");
        assert!(has(p.text), "readable text");
        // SAFETY: closing the dialog this test opened.
        unsafe { SendMessageW(d, WM_COMMAND, Some(WPARAM(2)), Some(LPARAM(0))) };
    });
    assert!(!answer.confirmed);
}
