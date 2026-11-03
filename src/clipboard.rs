//! Clipboard FFI boundary: `CF_UNICODETEXT` get/set (spec WIN-004).
//!
//! This module is a raw Win32 FFI boundary; unsafe is confined here under
//! the NFR-401 allow in `lib.rs`. Failures (clipboard held by another
//! process, allocation failure) are reported as `None`/`false` — the UI
//! must degrade silently, never panic.

use windows::Win32::Foundation::HWND;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// Read the clipboard text, or `None` when unavailable/empty/not text.
pub fn get_text() -> Option<String> {
    unsafe {
        OpenClipboard(None).ok()?;
        let mut result = None;
        if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok()
            && let Ok(handle) = GetClipboardData(CF_UNICODETEXT.0 as u32)
        {
            // The clipboard hands back the block as a HANDLE; it is the
            // same pointer the Global* family allocates.
            let block = windows::Win32::Foundation::HGLOBAL(handle.0);
            let ptr = GlobalLock(block).cast::<u16>();
            if !ptr.is_null() {
                let len = GlobalSize(block) / 2;
                let slice = std::slice::from_raw_parts(ptr, len);
                let end = slice.iter().position(|&c| c == 0).unwrap_or(len);
                result = Some(String::from_utf16_lossy(&slice[..end]));
                let _ = GlobalUnlock(block);
            }
        }
        let _ = CloseClipboard();
        result
    }
}

/// Write `text` onto the clipboard as `CF_UNICODETEXT`.
///
/// Returns `false` when the clipboard is held by another process or the
/// system refuses the write.
pub fn set_text(owner: HWND, text: &str) -> bool {
    unsafe {
        if OpenClipboard(Some(owner)).is_err() {
            return false;
        }
        let ok = (|| {
            EmptyClipboard().ok()?;
            let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let handle = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2).ok()?;
            let ptr = GlobalLock(handle).cast::<u16>();
            if ptr.is_null() {
                let _ = windows::Win32::Foundation::GlobalFree(Some(handle));
                return None;
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), ptr, units.len());
            // GlobalUnlock returns FALSE on the final unlock of a singly
            // locked block (with ERROR_NOT_LOCKED) — that is success for us,
            // so its BOOL must not be treated as an error.
            let _ = GlobalUnlock(handle);
            // On success the system owns the block from here on.
            SetClipboardData(
                CF_UNICODETEXT.0 as u32,
                Some(windows::Win32::Foundation::HANDLE(handle.0)),
            )
            .ok()?;
            Some(())
        })()
        .is_some();
        let _ = CloseClipboard();
        ok
    }
}
