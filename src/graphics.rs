//! Kitty graphics protocol: header parsing and base64 (spec "graphics
//! protocols", roadmap v0.5).
//!
//! The protocol rides on APC (`ESC _ G ... ESC \`) and DCS (`ESC P G ...`)
//! strings; the parser captures the payload and [`parse_kitty`] turns it
//! into a [`GraphicsAction`]. This module is pure Rust: it decodes the
//! base64 payload and the header keys, nothing Windows-specific.
//!
//! Supported subset: transmit (`a=t`, chunked with `m=1/2`), placement
//! (`a=p`), delete (`a=d`, all or by id), and queries (`a=I`/`a=Q`).
//! PNG and JPEG payloads stay encoded (the renderer decodes them with WIC);
//! `f=24`/`f=32` are raw RGB/RGBA pixels and need `w=`/`h=`.
//!
//! Out of scope (shortcut: nothing in the wild uses these from a Windows
//! terminal; upgrade if a real app asks): compression (`z=i`/`z=o`),
//! file storage (`t=f`), animation frames (`a=f`), cell cropping (`s=`/`v=`),
//! unicode placeholders, and z-index layering (every placement draws above
//! the text).

/// One decoded kitty graphics command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphicsAction {
    /// `a=t`: payload received, base64 already decoded.
    Transmit {
        /// Image id (`0` = the pane assigns one).
        id: u32,
        /// `f=`: 100 PNG, 101 JPEG, 24 RGB, 32 RGBA.
        format: u32,
        /// `w=`/`h=` in pixels (raw formats only).
        width: u32,
        /// `h=` in pixels (raw formats only).
        height: u32,
        /// Decoded payload bytes for this chunk.
        data: Vec<u8>,
        /// `m=1`: more chunks follow; `m=2`: complete, do not display.
        more: u8,
    },
    /// `a=p`: show an image at the cursor, cursor-relative.
    Place {
        /// Image id to show.
        id: u32,
        /// `x=`: column offset from the cursor.
        x: i32,
        /// `y=`: row offset from the cursor.
        y: i32,
        /// `c=`: columns to cover (`0` = native pixel width).
        cols: u16,
        /// `r=`: rows to cover (`0` = native pixel height).
        rows: u16,
        /// `t=1`: transient — dropped when it scrolls out of view.
        transient: bool,
    },
    /// `a=d` with no `q=i`/`i=`: drop every image and placement.
    DeleteAll,
    /// `a=d,q=i` / `a=d,i=N`: drop image `N` and its placements.
    DeleteId(u32),
    /// `a=I` / `a=Q`: capability query (`None`) or image query (`Some(id)`).
    Query(Option<u32>),
}

/// Decode the base64 payload of a kitty transmission (standard alphabet;
/// padding and anything outside the alphabet are ignored).
#[must_use]
pub fn base64_decode(data: &str) -> Vec<u8> {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(data.len() / 4 * 3 + 3);
    let mut acc = 0u32;
    let mut n = 0u8;
    for &b in data.as_bytes() {
        let Some(v) = val(b) else { continue };
        acc = acc << 6 | u32::from(v);
        n += 1;
        if n == 4 {
            out.extend_from_slice(&acc.to_be_bytes()[1..]);
            acc = 0;
            n = 0;
        }
    }
    // Trailing 2/3 characters carry 1/2 final bytes.
    if n >= 2 {
        let extra = (n - 2) as usize;
        let bytes = (acc << (6 * (4 - n) as u32)).to_be_bytes();
        out.extend_from_slice(&bytes[1..2 + extra]);
    }
    out
}

/// Parse one kitty payload (`a=t,f=100,i=3;base64...`); `None` when the
/// header is malformed.
#[must_use]
pub fn parse_kitty(payload: &[u8]) -> Option<GraphicsAction> {
    let text = String::from_utf8_lossy(payload);
    let (meta, data) = text.split_once(';').unwrap_or((text.as_ref(), ""));
    let mut action = String::new();
    let (mut id, mut format, mut more) = (0u32, 100u32, 0u8);
    let (mut width, mut height) = (0u32, 0u32);
    let (mut x, mut y, mut cols, mut rows) = (0i32, 0i32, 0u16, 0u16);
    let mut transient = false;
    let mut delete_all = false;
    for kv in meta.split(',') {
        let (key, value) = kv.split_once('=')?;
        if key == "a" {
            action = value.to_ascii_lowercase();
            continue;
        }
        let number = value.parse::<i64>().ok()?;
        match key {
            "i" => id = u32::try_from(number).ok()?,
            "f" => format = u32::try_from(number).ok()?,
            "m" => more = u8::try_from(number).ok()?,
            "w" => width = u32::try_from(number).ok()?,
            "h" => height = u32::try_from(number).ok()?,
            "x" => x = i32::try_from(number).ok()?,
            "y" => y = i32::try_from(number).ok()?,
            "c" => cols = u16::try_from(number).ok()?,
            "r" => rows = u16::try_from(number).ok()?,
            "t" => transient = number == 1,
            "z" => {} // shortcut: every placement draws above the text
            "q" => delete_all = value.eq_ignore_ascii_case("a"),
            "s" | "v" | "S" => {} // cropping: out of scope
            _ => {}
        }
    }
    // The spec also allows numeric action codes (0=transmit, 1=put,
    // 2=delete, 3/8=query, 9=image query).
    let action = match action.as_str() {
        "" | "0" | "t" => "t",
        "1" | "p" => "p",
        "2" | "d" => "d",
        "3" | "8" | "i" => "I",
        "9" | "q" => "Q",
        _ => return None,
    };
    Some(match action {
        "t" => GraphicsAction::Transmit {
            id,
            format,
            width,
            height,
            data: base64_decode(data),
            more,
        },
        "p" => GraphicsAction::Place {
            id,
            x,
            y,
            cols,
            rows,
            transient,
        },
        "d" => {
            if delete_all || id == 0 {
                GraphicsAction::DeleteAll
            } else {
                GraphicsAction::DeleteId(id)
            }
        }
        "I" => GraphicsAction::Query(None),
        _ => GraphicsAction::Query(if id == 0 { None } else { Some(id) }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_std_alphabet() {
        assert_eq!(base64_decode(""), Vec::<u8>::new());
        assert_eq!(base64_decode("aGk="), b"hi".to_vec());
        assert_eq!(base64_decode("aGVsbG8="), b"hello".to_vec());
        assert_eq!(base64_decode("TUFOTUFO"), b"MANMAN".to_vec());
        // 1 and 2 leftover characters decode to 0 and 1 bytes.
        assert_eq!(base64_decode("aGk"), b"hi".to_vec());
        assert_eq!(base64_decode("YQ"), b"a".to_vec());
    }

    #[test]
    fn transmit_header_and_payload() {
        let action = parse_kitty(b"a=t,f=100,i=7;YWJj").expect("parses");
        assert_eq!(
            action,
            GraphicsAction::Transmit {
                id: 7,
                format: 100,
                width: 0,
                height: 0,
                data: b"abc".to_vec(),
                more: 0,
            }
        );
    }

    #[test]
    fn chunked_transmit_carries_more() {
        let action = parse_kitty(b"a=0,m=1,i=4;YQ").expect("parses");
        assert_eq!(
            action,
            GraphicsAction::Transmit {
                id: 4,
                format: 100,
                width: 0,
                height: 0,
                data: b"a".to_vec(),
                more: 1,
            }
        );
    }

    #[test]
    fn raw_rgba_needs_size() {
        let action = parse_kitty(b"a=t,f=32,w=2,h=1,m=2;AAECAw==").expect("parses");
        let GraphicsAction::Transmit {
            format,
            width,
            height,
            data,
            more,
            ..
        } = action
        else {
            panic!("transmit");
        };
        assert_eq!((format, width, height, more), (32, 2, 1, 2));
        assert_eq!(data, vec![0, 1, 2, 3]);
    }

    #[test]
    fn placement_and_delete_and_query() {
        assert_eq!(
            parse_kitty(b"a=p,i=3,x=0,y=-1,c=10,r=5,t=1").expect("parses"),
            GraphicsAction::Place {
                id: 3,
                x: 0,
                y: -1,
                cols: 10,
                rows: 5,
                transient: true,
            }
        );
        assert_eq!(
            parse_kitty(b"a=d").expect("parses"),
            GraphicsAction::DeleteAll
        );
        assert_eq!(
            parse_kitty(b"a=d,i=9").expect("parses"),
            GraphicsAction::DeleteId(9)
        );
        assert_eq!(
            parse_kitty(b"a=I").expect("parses"),
            GraphicsAction::Query(None)
        );
        assert_eq!(
            parse_kitty(b"a=Q,i=2").expect("parses"),
            GraphicsAction::Query(Some(2))
        );
    }

    #[test]
    fn malformed_headers_are_dropped() {
        assert_eq!(parse_kitty(b"a=t,i=nope"), None, "id must be a number");
        assert_eq!(parse_kitty(b"a=z;"), None, "unknown action");
        assert_eq!(parse_kitty(b"nonsense"), None, "no key=value");
    }
}
