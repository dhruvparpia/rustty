//! Keyboard encoding: virtual-key codes + modifiers to VT byte sequences
//! (spec WIN-005).
//!
//! This module is deliberately pure Rust with no `windows` dependency: the
//! window layer reads Ctrl/Alt/Shift with `GetKeyState`, passes them in as
//! [`Modifiers`], and forwards the returned bytes straight to the shell.
//! Keeping the encoding here makes it unit-testable without a window.

/// Ctrl/Alt/Shift state for one key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    /// Ctrl held (left or right).
    pub ctrl: bool,
    /// Alt (menu key) held.
    pub alt: bool,
    /// Shift held.
    pub shift: bool,
}

// Win32 virtual-key codes we encode, spelled out as plain `u16` so this
// module stays free of the `windows` crate.
/// Left arrow.
pub const VK_LEFT: u16 = 0x25;
/// Up arrow.
pub const VK_UP: u16 = 0x26;
/// Right arrow.
pub const VK_RIGHT: u16 = 0x27;
/// Down arrow.
pub const VK_DOWN: u16 = 0x28;
/// Page Up.
pub const VK_PRIOR: u16 = 0x21;
/// Page Down.
pub const VK_NEXT: u16 = 0x22;
/// End.
pub const VK_END: u16 = 0x23;
/// Home.
pub const VK_HOME: u16 = 0x24;
/// Insert.
pub const VK_INSERT: u16 = 0x2D;
/// Delete.
pub const VK_DELETE: u16 = 0x2E;
/// F1.
pub const VK_F1: u16 = 0x70;

/// xterm modifier parameter: 1 + shift(1) + ctrl(4).
///
/// Alt is deliberately absent: this terminal sends Alt as the `ESC` prefix
/// (xterm convention), so it must not also appear as a `;3` parameter.
fn modifier_code(mods: Modifiers) -> u8 {
    1 + u8::from(mods.shift) + 4 * u8::from(mods.ctrl)
}

/// Encode a function/navigation key press (from `WM_KEYDOWN`).
///
/// Returns an empty vector for keys with no VT encoding (letters, digits,
/// and text keys are handled from `WM_CHAR` via [`encode_char`], so they
/// must not be sent twice).
///
/// * `application_cursor_keys` — DECCKM (`CSI ?1h`): arrows send `ESC O A`
///   style sequences instead of `ESC [ A`.
pub fn encode_key(vk: u16, mods: Modifiers, application_cursor_keys: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if mods.alt {
        // xterm sends Alt-keyed sequences as ESC prefix + plain sequence.
        out.push(0x1B);
    }
    // Arrow keys and Home/End: CSI letter, DECCKM variant, modifier param.
    let letter = match vk {
        VK_LEFT => Some(b'D'),
        VK_UP => Some(b'A'),
        VK_RIGHT => Some(b'C'),
        VK_DOWN => Some(b'B'),
        VK_HOME => Some(b'H'),
        VK_END => Some(b'F'),
        _ => None,
    };
    if let Some(letter) = letter {
        // Alt is encoded as the ESC prefix above, never also as a `;3`
        // parameter (xterm convention: ESC prefix + plain sequence).
        let modded = mods.ctrl || mods.shift;
        if modded {
            out.extend_from_slice(b"\x1b[1;");
            out.push(b'0' + modifier_code(mods));
            out.push(letter);
        } else if application_cursor_keys {
            out.extend_from_slice(b"\x1bO");
            out.push(letter);
        } else {
            out.extend_from_slice(b"\x1b[");
            out.push(letter);
        }
        return out;
    }
    // Tilde sequences: PageUp/PgDn/Insert/Delete and F5..F12.
    let tilde = match vk {
        VK_PRIOR => Some(b"5" as &[u8]),
        VK_NEXT => Some(b"6" as &[u8]),
        VK_INSERT => Some(b"2" as &[u8]),
        VK_DELETE => Some(b"3" as &[u8]),
        0x74 => Some(b"15" as &[u8]), // F5
        0x75 => Some(b"17" as &[u8]), // F6
        0x76 => Some(b"18" as &[u8]), // F7
        0x77 => Some(b"19" as &[u8]), // F8
        0x78 => Some(b"20" as &[u8]), // F9
        0x79 => Some(b"21" as &[u8]), // F10
        0x7A => Some(b"23" as &[u8]), // F11
        0x7B => Some(b"24" as &[u8]), // F12
        _ => None,
    };
    if let Some(code) = tilde {
        out.extend_from_slice(b"\x1b[");
        out.extend_from_slice(code);
        if mods.ctrl || mods.shift {
            out.push(b';');
            out.push(b'0' + modifier_code(mods));
        }
        out.push(b'~');
        return out;
    }
    // F1..F4: SS3 letters (plain) or CSI 1;m letters (modified).
    let ss3 = match vk {
        VK_F1 => Some(b'P'),
        0x71 => Some(b'Q'), // F2
        0x72 => Some(b'R'), // F3
        0x73 => Some(b'S'), // F4
        _ => None,
    };
    if let Some(letter) = ss3 {
        if mods.ctrl || mods.shift {
            out.extend_from_slice(b"\x1b[1;");
            out.push(b'0' + modifier_code(mods));
            out.push(letter);
        } else {
            out.extend_from_slice(b"\x1bO");
            out.push(letter);
        }
        return out;
    }
    Vec::new()
}

/// Encode a character event (from `WM_CHAR`).
///
/// Control characters from the OS (Ctrl+letter arrives as its control byte)
/// pass through; Alt-prefixed text sends `ESC` + plain text; everything
/// else is UTF-8. Backspace is normalized to `0x7F` (the VT convention).
#[must_use]
pub fn encode_char(ch: char, mods: Modifiers) -> Vec<u8> {
    let mut out = Vec::new();
    if mods.alt {
        out.push(0x1B);
    }
    let code = u32::from(ch);
    if code == 0x08 {
        out.push(0x7F); // VT convention: backspace is DEL
    } else if code < 0x20 || code == 0x7F {
        out.push(u8::try_from(code).unwrap_or(b' ')); // control byte passes through
    } else {
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
    out
}

/// Decode UTF-16 code units (an IME composition string, WIN-006) to
/// characters. A high surrogate pairs with the next low surrogate; unpaired
/// surrogates are dropped, so a partial string never sends garbage.
#[must_use]
pub fn utf16_to_chars(units: &[u16]) -> Vec<char> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < units.len() {
        let unit = units[i];
        if (0xD800..0xDC00).contains(&unit)
            && i + 1 < units.len()
            && (0xDC00..0xE000).contains(&units[i + 1])
        {
            let combined = u32::from(unit) - 0xD800;
            let low = u32::from(units[i + 1]) - 0xDC00;
            if let Some(ch) = char::from_u32(0x1_0000 + (combined << 10) + low) {
                out.push(ch);
            }
            i += 2;
        } else if let Some(ch) = char::from_u32(u32::from(unit)) {
            out.push(ch);
            i += 1;
        } else {
            i += 1; // unpaired surrogate
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, alt: bool, shift: bool) -> Modifiers {
        Modifiers { ctrl, alt, shift }
    }

    #[test]
    fn plain_arrows_use_csi() {
        assert_eq!(encode_key(VK_UP, Modifiers::default(), false), b"\x1b[A");
    }

    #[test]
    fn ime_utf16_decodes_pairs_and_drops_lone_surrogates() {
        // U+3053 (hiragana ko), a valid astral pair (U+20BB7), then a high
        // surrogate followed by a BMP unit and a trailing lone low surrogate:
        // both halves are dropped.
        let units = [0x3053u16, 0xD842, 0xDFB7, 0xD800, 0x3053, 0xDC00];
        assert_eq!(utf16_to_chars(&units), vec!['こ', '\u{20BB7}', 'こ']);
        assert_eq!(utf16_to_chars(&[]), Vec::<char>::new());
    }

    #[test]
    fn application_cursor_keys_use_ss3() {
        assert_eq!(encode_key(VK_UP, Modifiers::default(), true), b"\x1bOA");
    }

    #[test]
    fn modified_arrows_carry_modifier_param() {
        // Ctrl+Shift+Left: 1 + shift + ctrl = 6.
        assert_eq!(
            encode_key(VK_LEFT, mods(true, false, true), false),
            b"\x1b[1;6D"
        );
    }

    #[test]
    fn alt_prefixes_the_sequence() {
        assert_eq!(
            encode_key(VK_DELETE, mods(false, true, false), false),
            b"\x1b\x1b[3~"
        );
    }

    #[test]
    fn alt_never_appears_in_the_modifier_param() {
        // Ctrl+Alt+Delete: ESC prefix carries Alt, the param carries Ctrl.
        assert_eq!(
            encode_key(VK_DELETE, mods(true, true, false), false),
            b"\x1b\x1b[3;5~"
        );
    }

    #[test]
    fn f_keys_encode() {
        assert_eq!(encode_key(VK_F1, Modifiers::default(), false), b"\x1bOP");
        assert_eq!(
            encode_key(0x74, mods(false, false, true), false),
            b"\x1b[15;2~"
        );
    }

    #[test]
    fn pgup_pgdn_encode() {
        assert_eq!(
            encode_key(VK_PRIOR, Modifiers::default(), false),
            b"\x1b[5~"
        );
        assert_eq!(
            encode_key(VK_NEXT, mods(true, false, false), false),
            b"\x1b[6;5~"
        );
    }

    #[test]
    fn text_keys_are_not_encoded_here() {
        assert!(encode_key(0x41, Modifiers::default(), false).is_empty());
    }

    #[test]
    fn chars_encode_as_utf8() {
        assert_eq!(encode_char('a', Modifiers::default()), b"a");
        assert_eq!(encode_char('ü', Modifiers::default()), "ü".as_bytes());
    }

    #[test]
    fn backspace_is_del_and_controls_pass_through() {
        assert_eq!(encode_char('\u{8}', Modifiers::default()), vec![0x7F]);
        assert_eq!(encode_char('\r', Modifiers::default()), b"\r");
        assert_eq!(encode_char('\u{3}', Modifiers::default()), vec![3]);
    }

    #[test]
    fn alt_chars_get_escape_prefix() {
        assert_eq!(encode_char('a', mods(false, true, false)), b"\x1ba");
    }
}
