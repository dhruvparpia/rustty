//! Skinned dialogs (DIALOG-001): every question and message rustty asks, in the
//! theme's colors. They are real Win32 dialogs with real controls, so the keyboard,
//! screen readers, dragging and the window's shadow all stay native. Only the
//! painting is ours. The Save dialog is the system's: Windows does not let an app
//! skin it.

use crate::color::Color;
use crate::theme::Theme;

/// The colors a dialog is painted with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// The dialog's body and its title bar.
    pub surface: Color,
    /// The strip the buttons sit on.
    pub footer: Color,
    /// Text on the surface.
    pub text: Color,
    /// The line above the footer, and the outline of quiet buttons.
    pub border: Color,
    /// The confirm button.
    pub accent: Color,
    /// Text on the confirm button.
    pub on_accent: Color,
    /// The confirm button when it destroys something.
    pub danger: Color,
    /// Text on that button.
    pub on_danger: Color,
    /// A text field's background.
    pub field: Color,
    /// Whether the title bar should be dark.
    pub dark: bool,
}

/// WCAG relative luminance.
fn luminance(c: Color) -> f64 {
    let channel = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
}

/// WCAG contrast ratio.
fn contrast_ratio(a: Color, b: Color) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Black or white, whichever reads better on `background`. One of them always
/// reaches 4.5:1.
#[must_use]
pub fn readable_on(background: Color) -> Color {
    // Where black and white have equal contrast: sqrt(1.05 * 0.05) - 0.05.
    if luminance(background) > 0.179 {
        Color::rgb(0, 0, 0)
    } else {
        Color::rgb(255, 255, 255)
    }
}

/// The dialog colors for a theme.
#[must_use]
pub fn palette(theme: &Theme) -> Palette {
    let ui = theme.ui_colors();
    let surface = theme.tab_bar;
    let danger = theme.palette[1];
    let brightness =
        u32::from(surface.r) * 299 + u32::from(surface.g) * 587 + u32::from(surface.b) * 114;
    let footer = surface.mix(ui.foreground, 0.04);
    let field = theme.background;
    // A dialog has to be read: not every theme's chrome text is strong enough.
    let reads_everywhere = [surface, footer, field]
        .iter()
        .all(|&on| contrast_ratio(ui.foreground, on) >= 4.5);
    let text = if reads_everywhere {
        ui.foreground
    } else {
        readable_on(surface)
    };
    Palette {
        surface,
        footer,
        text,
        border: ui.border,
        accent: ui.accent,
        on_accent: readable_on(ui.accent),
        danger,
        on_danger: readable_on(danger),
        field,
        dark: brightness < 128_000,
    }
}

/// A rectangle in pixels: left, top, width, height.
pub type Rect = (i32, i32, i32, i32);

/// Where everything goes, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// The client area's width and height.
    pub client: (i32, i32),
    /// The app icon.
    pub icon: Rect,
    /// The bold first line, when there is one.
    pub heading: Option<Rect>,
    /// The message.
    pub message: Rect,
    /// The text field, when there is one.
    pub field: Option<Rect>,
    /// Where the footer starts.
    pub footer_top: i32,
    /// The confirm button.
    pub confirm: Rect,
    /// The cancel button, when there is one.
    pub cancel: Option<Rect>,
}

/// What the layout has to fit, in pixels at `dpi`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measured {
    /// The display's DPI.
    pub dpi: u32,
    /// The heading's height, 0 when there is none.
    pub heading_height: i32,
    /// The message's height.
    pub message_height: i32,
    /// Whether there is a text field.
    pub has_field: bool,
    /// The confirm label's width.
    pub confirm_label: i32,
    /// The cancel label's width, when there is a cancel button.
    pub cancel_label: Option<i32>,
    /// The tallest the client area may be.
    pub max_height: i32,
}

/// The dialog's width in DIPs. Its message wraps to fit.
pub const WIDTH: i32 = 420;

/// The width left for text, in pixels.
#[must_use]
pub fn text_width(dpi: u32) -> i32 {
    px(WIDTH - MARGIN - ICON - ICON_GAP - MARGIN, dpi)
}

/// DIPs to pixels.
const fn px(dips: i32, dpi: u32) -> i32 {
    dips * dpi as i32 / 96
}

const MARGIN: i32 = 20;
const ICON: i32 = 32;
const ICON_GAP: i32 = 14;
const HEADING_GAP: i32 = 6;
const FIELD_GAP: i32 = 12;
const FIELD_HEIGHT: i32 = 28;
const FOOTER_PADDING: i32 = 12;
const FOOTER_EDGE: i32 = 16;
const BUTTON_HEIGHT: i32 = 30;
const BUTTON_MIN_WIDTH: i32 = 88;
const BUTTON_LABEL_PADDING: i32 = 16;
const BUTTON_GAP: i32 = 12;

/// Lay a dialog out.
#[must_use]
pub fn layout(m: &Measured) -> Layout {
    let dpi = m.dpi;
    let button_width =
        |label: i32| (label + 2 * px(BUTTON_LABEL_PADDING, dpi)).max(px(BUTTON_MIN_WIDTH, dpi));
    let confirm_width = button_width(m.confirm_label);
    let cancel_width = m.cancel_label.map(button_width);
    let buttons = confirm_width + cancel_width.map_or(0, |w| w + px(BUTTON_GAP, dpi));
    let width = px(WIDTH, dpi).max(buttons + 2 * px(FOOTER_EDGE, dpi));
    let text_left = px(MARGIN + ICON + ICON_GAP, dpi);
    let text = text_width(dpi);

    let heading_space = if m.heading_height > 0 {
        m.heading_height + px(HEADING_GAP, dpi)
    } else {
        0
    };
    let field_space = if m.has_field {
        px(FIELD_GAP + FIELD_HEIGHT, dpi)
    } else {
        0
    };
    let footer = px(2 * FOOTER_PADDING + BUTTON_HEIGHT, dpi);
    let margin = px(MARGIN, dpi);
    // A message too long for the screen is cut, so the buttons stay reachable.
    let room = m.max_height - footer - 2 * margin - heading_space - field_space;
    let message_height = m.message_height.min(room).max(px(16, dpi));
    let content = (heading_space + message_height + field_space).max(px(ICON, dpi));
    let footer_top = margin + content + margin;

    let heading = (m.heading_height > 0).then_some((text_left, margin, text, m.heading_height));
    let message = (text_left, margin + heading_space, text, message_height);
    let field = m.has_field.then_some((
        text_left,
        margin + heading_space + message_height + px(FIELD_GAP, dpi),
        text,
        px(FIELD_HEIGHT, dpi),
    ));
    let button_top = footer_top + px(FOOTER_PADDING, dpi);
    let height = px(BUTTON_HEIGHT, dpi);
    let right = width - px(FOOTER_EDGE, dpi);
    let cancel = cancel_width.map(|w| (right - w, button_top, w, height));
    let confirm_right = cancel.map_or(right, |c| c.0 - px(BUTTON_GAP, dpi));
    Layout {
        client: (width, footer_top + footer),
        icon: (margin, margin, px(ICON, dpi), px(ICON, dpi)),
        heading,
        message,
        field,
        footer_top,
        confirm: (
            confirm_right - confirm_width,
            button_top,
            confirm_width,
            height,
        ),
        cancel,
    }
}

/// The message's control id.
pub const ID_MESSAGE: u16 = 1001;
/// `IDOK`.
pub const ID_OK: u16 = 1;
/// `IDCANCEL`.
pub const ID_CANCEL: u16 = 2;
/// `IDYES`.
pub const ID_YES: u16 = 6;
/// `IDNO`.
pub const ID_NO: u16 = 7;

/// Which button a `WM_COMMAND` really means. Windows sends the *default* button's
/// id for Enter, even when another button has the focus, because a drawn button
/// cannot be a default push button. A person pressing Enter means the button they
/// are on. Esc and the close box always mean cancel.
#[must_use]
pub const fn answer_for(command: u16, from_control: bool, focused_button: Option<u16>) -> u16 {
    if command == ID_CANCEL {
        return ID_CANCEL;
    }
    match focused_button {
        Some(button) if from_control => button,
        _ => command,
    }
}

/// How much a dialog's confirm button should warn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// An ordinary confirm button.
    Normal,
    /// It destroys or kills something: the button says so, and the focus starts
    /// on cancel.
    Danger,
}

/// What a dialog says and offers. `&` in a label marks its keyboard letter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec<'a> {
    /// The window's title.
    pub title: &'a str,
    /// The bold first line.
    pub heading: Option<&'a str>,
    /// The message.
    pub message: &'a str,
    /// The confirm button's label.
    pub confirm: &'a str,
    /// The cancel button's label. `None` is a message with one button.
    pub cancel: Option<&'a str>,
    /// How much it warns.
    pub tone: Tone,
    /// A text field's starting text. `None` has no field.
    pub input: Option<&'a str>,
    /// Whether the focus starts on confirm, so Enter accepts. A question that
    /// could lose something starts on cancel.
    pub focus_confirm: bool,
}

impl<'a> Spec<'a> {
    /// A message with one button.
    #[must_use]
    pub const fn message(message: &'a str) -> Self {
        Self {
            title: "rustty",
            heading: None,
            message,
            confirm: "OK",
            cancel: None,
            tone: Tone::Normal,
            input: None,
            focus_confirm: true,
        }
    }

    /// A question whose focus starts on cancel, so a stray Enter changes nothing.
    #[must_use]
    pub const fn question(
        heading: Option<&'a str>,
        message: &'a str,
        confirm: &'a str,
        cancel: &'a str,
        tone: Tone,
    ) -> Self {
        Self {
            title: "rustty",
            heading,
            message,
            confirm,
            cancel: Some(cancel),
            tone,
            input: None,
            focus_confirm: false,
        }
    }
}

/// What the person did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// They chose confirm. Esc, the close box and cancel are all `false`.
    pub confirmed: bool,
    /// What the text field held, when they confirmed.
    pub text: Option<String>,
}

#[cfg(windows)]
pub use native::show;

#[cfg(windows)]
mod native {
    use std::ffi::c_void;

    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DwmSetWindowAttribute,
    };
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush,
        DEFAULT_CHARSET, DT_CALCRECT, DT_CENTER, DT_HIDEPREFIX, DT_NOPREFIX, DT_SINGLELINE,
        DT_VCENTER, DT_WORDBREAK, DeleteObject, DrawTextW, EndPaint, FF_DONTCARE, FW_NORMAL,
        FW_SEMIBOLD, FillRect, FrameRect, GetDC, GetMonitorInfoW, HBRUSH, HDC, HFONT, HGDIOBJ,
        MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromWindow,
        OUT_DEFAULT_PRECIS, PAINTSTRUCT, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
        TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_FOCUS, ODS_NOACCEL, ODS_SELECTED};
    use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetFocus, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, DI_NORMAL, DLGPROC, DLGTEMPLATE, DM_SETDEFID, DS_MODALFRAME,
        DS_SETFOREGROUND, DialogBoxIndirectParamW, DrawIconEx, EndDialog, GWL_EXSTYLE, GWL_STYLE,
        GWLP_USERDATA, GWLP_WNDPROC, GetDlgItem, GetParent, GetSystemMetrics, GetWindowLongPtrW,
        GetWindowRect, GetWindowTextW, HICON, IDYES, IsIconic, LoadIconW, MB_DEFBUTTON2,
        MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_YESNO, MessageBoxW, SM_CXSCREEN, SM_CYSCREEN,
        SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetWindowLongPtrW, SetWindowPos,
        WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND, WM_CTLCOLORDLG, WM_CTLCOLOREDIT,
        WM_CTLCOLORSTATIC, WM_DRAWITEM, WM_INITDIALOG, WM_MOUSEMOVE, WM_PAINT, WM_SETFONT,
        WS_CAPTION, WS_CHILD, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
    };
    use windows::core::{PCWSTR, w};

    use super::{
        Answer, ID_CANCEL, ID_MESSAGE, ID_NO, ID_OK, ID_YES, Layout, Measured, Palette, Rect, Spec,
        Tone, answer_for, layout, text_width,
    };
    use crate::color::Color;

    /// `BS_OWNERDRAW`: we paint the button; Windows keeps its behaviour.
    const BS_OWNERDRAW: u32 = 0x0000_000B;
    /// `SS_NOPREFIX`: a `&` in a message is a `&`.
    const SS_NOPREFIX: u32 = 0x0000_0080;
    /// `ES_AUTOHSCROLL`: a field keeps taking text past its right edge.
    const ES_AUTOHSCROLL: u32 = 0x0000_0080;
    /// `EM_SETSEL`: select the field's text, so typing replaces it.
    const EM_SETSEL: u32 = 0x00B1;
    /// `WM_MOUSELEAVE`, which the bindings do not export.
    const WM_MOUSELEAVE: u32 = 0x02A3;
    const ID_FIELD: u16 = 1000;
    const ID_HEADING: u16 = 1002;
    /// The class atoms a dialog template names its controls by.
    const CLASS_BUTTON: u16 = 0x0080;
    const CLASS_EDIT: u16 = 0x0081;
    const CLASS_STATIC: u16 = 0x0082;

    fn colorref(c: Color) -> COLORREF {
        COLORREF(u32::from(c.r) | (u32::from(c.g) << 8) | (u32::from(c.b) << 16))
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    /// A dialog's live state, reached from its window.
    struct Context<'a> {
        spec: &'a Spec<'a>,
        palette: Palette,
        surface: HBRUSH,
        footer: HBRUSH,
        field: HBRUSH,
        border: HBRUSH,
        font: HFONT,
        bold: HFONT,
        icon: Option<HICON>,
        layout: Option<Layout>,
        confirm_id: u16,
        cancel_id: u16,
        /// The button under the pointer.
        hovered: Option<u16>,
        /// What buttons do before our subclass.
        button_proc: isize,
        text: Option<String>,
    }

    /// The `DLGTEMPLATE` words: `style`, then one record per control, every field
    /// word-aligned and each record starting on a double word.
    fn push_str(words: &mut Vec<u16>, text: &str) {
        words.extend(text.encode_utf16().filter(|&unit| unit != 0));
        words.push(0);
    }

    fn push_item(words: &mut Vec<u16>, style: u32, id: u16, class: u16, text: &str) {
        if words.len() % 2 == 1 {
            words.push(0);
        }
        words.extend([
            (style & 0xFFFF) as u16,
            (style >> 16) as u16,
            0,
            0,
            0,
            0,
            1,
            1,
            id,
            0xFFFF,
            class,
        ]);
        push_str(words, text);
        words.push(0); // no creation data
    }

    fn template(spec: &Spec<'_>, confirm_id: u16, cancel_id: u16) -> Vec<u32> {
        let child = WS_CHILD.0 | WS_VISIBLE.0;
        let button = child | WS_TABSTOP.0 | BS_OWNERDRAW;
        let style = WS_POPUP.0
            | WS_CAPTION.0
            | WS_SYSMENU.0
            | DS_MODALFRAME as u32
            | DS_SETFOREGROUND as u32;
        let count = 2
            + u16::from(spec.heading.is_some())
            + u16::from(spec.input.is_some())
            + u16::from(spec.cancel.is_some());
        let mut words = vec![
            (style & 0xFFFF) as u16,
            (style >> 16) as u16,
            0,
            0,
            count,
            0,
            0,
            1,
            1,
            0, // no menu
            0, // the default dialog class
        ];
        push_str(&mut words, spec.title);
        if let Some(heading) = spec.heading {
            push_item(
                &mut words,
                child | SS_NOPREFIX,
                ID_HEADING,
                CLASS_STATIC,
                heading,
            );
        }
        push_item(
            &mut words,
            child | SS_NOPREFIX,
            ID_MESSAGE,
            CLASS_STATIC,
            spec.message,
        );
        if spec.input.is_some() {
            let edit = child | WS_TABSTOP.0 | ES_AUTOHSCROLL;
            push_item(&mut words, edit, ID_FIELD, CLASS_EDIT, "");
        }
        push_item(&mut words, button, confirm_id, CLASS_BUTTON, spec.confirm);
        if let Some(cancel) = spec.cancel {
            push_item(&mut words, button, cancel_id, CLASS_BUTTON, cancel);
        }
        words
            .chunks(2)
            .map(|c| u32::from(c[0]) | c.get(1).map_or(0, |w| u32::from(*w) << 16))
            .collect()
    }

    /// Ask, and wait for the answer. Windows' own box stands in if the dialog cannot
    /// be built, so a question always gets asked. A field can't be offered that way,
    /// so then it reads as cancel.
    pub fn show(owner: Option<HWND>, palette: &Palette, spec: &Spec<'_>) -> Answer {
        let (confirm_id, cancel_id) = if spec.cancel.is_some() && spec.input.is_none() {
            (ID_YES, ID_NO)
        } else {
            (ID_OK, ID_CANCEL)
        };
        // SAFETY: the brushes and fonts are deleted below, after the dialog ends.
        let mut context = unsafe {
            Context {
                spec,
                palette: *palette,
                surface: CreateSolidBrush(colorref(palette.surface)),
                footer: CreateSolidBrush(colorref(palette.footer)),
                field: CreateSolidBrush(colorref(palette.field)),
                border: CreateSolidBrush(colorref(palette.border)),
                font: HFONT::default(),
                bold: HFONT::default(),
                icon: None,
                layout: None,
                confirm_id,
                cancel_id,
                hovered: None,
                button_proc: 0,
                text: None,
            }
        };
        let words = template(spec, confirm_id, cancel_id);
        let module = unsafe { GetModuleHandleW(None) }.ok();
        let proc: DLGPROC = Some(dialog_proc);
        // SAFETY: the template and context outlive the modal call, which only
        // returns once the dialog is gone.
        let result = unsafe {
            DialogBoxIndirectParamW(
                module.map(Into::into),
                words.as_ptr().cast::<DLGTEMPLATE>(),
                owner,
                proc,
                LPARAM((&raw mut context) as isize),
            )
        };
        // SAFETY: nothing uses these any more; the dialog has been destroyed.
        unsafe {
            for brush in [
                context.surface,
                context.footer,
                context.field,
                context.border,
            ] {
                let _ = DeleteObject(HGDIOBJ::from(brush));
            }
            for font in [context.font, context.bold] {
                if !font.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ::from(font));
                }
            }
        }
        if result <= 0 {
            return fall_back(owner, spec);
        }
        let confirmed = result == confirm_id as isize;
        Answer {
            confirmed,
            text: if confirmed { context.text.take() } else { None },
        }
    }

    /// Windows' own box, for when ours could not be built.
    fn fall_back(owner: Option<HWND>, spec: &Spec<'_>) -> Answer {
        if spec.input.is_some() {
            return Answer {
                confirmed: false,
                text: None,
            };
        }
        let mut body = String::new();
        if let Some(heading) = spec.heading {
            body.push_str(heading);
            body.push_str("\n\n");
        }
        body.push_str(spec.message);
        let (body, title) = (wide(&body), wide(spec.title));
        let style = if spec.cancel.is_some() {
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2 | MB_SETFOREGROUND
        } else {
            MB_OK | MB_ICONWARNING | MB_SETFOREGROUND
        };
        // SAFETY: both strings are NUL-terminated and outlive the call.
        let answer =
            unsafe { MessageBoxW(owner, PCWSTR(body.as_ptr()), PCWSTR(title.as_ptr()), style) };
        Answer {
            confirmed: spec.cancel.is_none() || answer == IDYES,
            text: None,
        }
    }

    fn context_of<'a>(hwnd: HWND) -> Option<&'a mut Context<'a>> {
        // SAFETY: the pointer is set in WM_INITDIALOG to a context that outlives
        // the modal call, and only this thread uses it.
        unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Context<'a>).as_mut() }
    }

    fn font_for(dpi: u32, weight: u32, points: i32) -> HFONT {
        let height = -(points * dpi as i32 * 96 / 72 / 96);
        // SAFETY: the face name is a static string; the font is deleted in `show`.
        unsafe {
            CreateFontW(
                height,
                0,
                0,
                0,
                weight as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                FF_DONTCARE.0.into(),
                w!("Segoe UI"),
            )
        }
    }

    /// How tall `text` wraps to, or how wide it is on one line.
    fn measure(hdc: HDC, font: HFONT, text: &str, wrap_width: Option<i32>) -> (i32, i32) {
        let mut units: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT {
            right: wrap_width.unwrap_or(10_000),
            ..RECT::default()
        };
        let flags = match wrap_width {
            Some(_) => DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
            None => DT_CALCRECT | DT_SINGLELINE,
        };
        // SAFETY: the font stays selected only for this call; the DC is valid.
        unsafe {
            let old = SelectObject(hdc, HGDIOBJ::from(font));
            DrawTextW(hdc, &mut units, &raw mut rect, flags);
            SelectObject(hdc, old);
        }
        (rect.right - rect.left, rect.bottom - rect.top)
    }

    /// Color the title bar to match. Windows 10 ignores what it doesn't know.
    fn skin_frame(hwnd: HWND, p: &Palette) {
        for (attribute, value) in [
            (DWMWA_USE_IMMERSIVE_DARK_MODE, u32::from(p.dark)),
            (DWMWA_CAPTION_COLOR, colorref(p.surface).0),
            (DWMWA_TEXT_COLOR, colorref(p.text).0),
            (DWMWA_BORDER_COLOR, colorref(p.border).0),
        ] {
            // SAFETY: each attribute takes four bytes, copied during the call.
            let _ = unsafe {
                DwmSetWindowAttribute(hwnd, attribute, (&raw const value).cast::<c_void>(), 4)
            };
        }
    }

    fn place(hwnd: HWND, id: u16, rect: Rect, font: HFONT) {
        // SAFETY: the control belongs to this dialog; a missing one is skipped.
        unsafe {
            if let Ok(control) = GetDlgItem(Some(hwnd), i32::from(id)) {
                let _ = SetWindowPos(
                    control,
                    None,
                    rect.0,
                    rect.1,
                    rect.2,
                    rect.3,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                SendMessageW(
                    control,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
        }
    }

    /// The screen area to stay inside, and the rectangle to center on.
    fn placement(hwnd: HWND, owner: Option<HWND>) -> (RECT, RECT) {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..MONITORINFO::default()
        };
        // SAFETY: plain queries against live windows, with sized out-parameters.
        unsafe {
            let monitor = match owner {
                Some(owner) => MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
                None => MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTONEAREST),
            };
            let work = if GetMonitorInfoW(monitor, &raw mut info).as_bool() {
                info.rcWork
            } else {
                RECT {
                    right: GetSystemMetrics(SM_CXSCREEN),
                    bottom: GetSystemMetrics(SM_CYSCREEN),
                    ..RECT::default()
                }
            };
            let mut over = work;
            if let Some(owner) = owner
                && !IsIconic(owner).as_bool()
                && GetWindowRect(owner, &raw mut over).is_err()
            {
                over = work;
            }
            let _ = hwnd;
            (work, over)
        }
    }

    fn set_up(hwnd: HWND, context: &mut Context<'_>) {
        // SAFETY: all handles belong to this dialog, on this thread.
        unsafe {
            skin_frame(hwnd, &context.palette);
            let dpi = GetDpiForWindow(hwnd).max(96);
            context.font = font_for(dpi, FW_NORMAL.0, 10);
            context.bold = font_for(dpi, FW_SEMIBOLD.0, 11);
            let module = GetModuleHandleW(None).ok();
            context.icon = LoadIconW(
                module.map(Into::into),
                PCWSTR(std::ptr::without_provenance::<u16>(1)),
            )
            .ok();

            let owner = GetParent(hwnd).ok();
            let (work, over) = placement(hwnd, owner);
            let dc = GetDC(Some(hwnd));
            let width = text_width(dpi);
            let spec = context.spec;
            let heading_height = spec
                .heading
                .map_or(0, |text| measure(dc, context.bold, text, Some(width)).1);
            let message_height = measure(dc, context.font, spec.message, Some(width)).1;
            let confirm_label = measure(dc, context.font, spec.confirm, None).0;
            let cancel_label = spec.cancel.map(|t| measure(dc, context.font, t, None).0);
            ReleaseDC(Some(hwnd), dc);

            let style = WINDOW_STYLE(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32);
            let ex_style = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
            let mut frame = RECT::default();
            let _ = AdjustWindowRectExForDpi(&raw mut frame, style, false, ex_style, dpi);
            let frame_height = frame.bottom - frame.top;
            let l = layout(&Measured {
                dpi,
                heading_height,
                message_height,
                has_field: spec.input.is_some(),
                confirm_label,
                cancel_label,
                max_height: (work.bottom - work.top - frame_height).max(1),
            });
            let outer = (
                l.client.0 + frame.right - frame.left,
                l.client.1 + frame_height,
            );
            let x = over.left + ((over.right - over.left) - outer.0) / 2;
            let y = over.top + ((over.bottom - over.top) - outer.1) / 2;
            let _ = SetWindowPos(
                hwnd,
                None,
                x.clamp(work.left, (work.right - outer.0).max(work.left)),
                y.clamp(work.top, (work.bottom - outer.1).max(work.top)),
                outer.0,
                outer.1,
                SWP_NOZORDER,
            );
            if let Some(rect) = l.heading {
                place(hwnd, ID_HEADING, rect, context.bold);
            }
            place(hwnd, ID_MESSAGE, l.message, context.font);
            if let Some(rect) = l.field {
                // An edit control's text sits at its top, so the control is one
                // line tall, centered in the box painted around it.
                let dc = GetDC(Some(hwnd));
                let line = measure(dc, context.font, "Ag", None).1;
                ReleaseDC(Some(hwnd), dc);
                let inset = dpi as i32 * 8 / 96;
                place(
                    hwnd,
                    ID_FIELD,
                    (
                        rect.0 + inset,
                        rect.1 + (rect.3 - line) / 2,
                        rect.2 - 2 * inset,
                        line,
                    ),
                    context.font,
                );
            }
            place(hwnd, context.confirm_id, l.confirm, context.font);
            if let Some(rect) = l.cancel {
                place(hwnd, context.cancel_id, rect, context.font);
            }
            context.layout = Some(l);

            for id in [context.confirm_id, context.cancel_id] {
                if let Ok(button) = GetDlgItem(Some(hwnd), i32::from(id)) {
                    context.button_proc =
                        SetWindowLongPtrW(button, GWLP_WNDPROC, button_proc as *const () as isize);
                }
            }
            let first = if spec.input.is_some() {
                ID_FIELD
            } else if spec.focus_confirm || spec.cancel.is_none() {
                context.confirm_id
            } else {
                context.cancel_id
            };
            let default = if first == ID_FIELD {
                context.confirm_id
            } else {
                first
            };
            SendMessageW(
                hwnd,
                DM_SETDEFID,
                Some(WPARAM(usize::from(default))),
                Some(LPARAM(0)),
            );
            if let Ok(control) = GetDlgItem(Some(hwnd), i32::from(first)) {
                let _ = SetFocus(Some(control));
                if let Some(seed) = spec.input {
                    let text = wide(seed);
                    let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                        control,
                        PCWSTR(text.as_ptr()),
                    );
                    SendMessageW(control, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                }
            }
        }
    }

    fn paint(hwnd: HWND, context: &Context<'_>) {
        let Some(l) = context.layout else { return };
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: BeginPaint and EndPaint are paired; handles live for the dialog.
        unsafe {
            let dc = BeginPaint(hwnd, &raw mut ps);
            let client = RECT {
                right: l.client.0,
                bottom: l.client.1,
                ..RECT::default()
            };
            let mut footer = client;
            footer.top = l.footer_top;
            FillRect(dc, &raw const footer, context.footer);
            let mut line = footer;
            line.bottom = line.top + 1;
            FillRect(dc, &raw const line, context.border);
            if let Some(icon) = context.icon {
                let _ = DrawIconEx(
                    dc, l.icon.0, l.icon.1, icon, l.icon.2, l.icon.3, 0, None, DI_NORMAL,
                );
            }
            if let Some(field) = l.field {
                let edge = RECT {
                    left: field.0,
                    top: field.1,
                    right: field.0 + field.2,
                    bottom: field.1 + field.3,
                };
                FillRect(dc, &raw const edge, context.field);
                FrameRect(dc, &raw const edge, context.border);
            }
            let _ = EndPaint(hwnd, &raw const ps);
        }
    }

    fn mix(a: Color, b: Color, amount: u16) -> Color {
        let part =
            |x: u8, y: u8| ((u16::from(x) * (100 - amount) + u16::from(y) * amount) / 100) as u8;
        Color::rgb(part(a.r, b.r), part(a.g, b.g), part(a.b, b.b))
    }

    fn draw_button(item: &DRAWITEMSTRUCT, context: &Context<'_>) {
        let id = item.CtlID as u16;
        let p = &context.palette;
        let primary = id == context.confirm_id;
        let (fill, ink) = match (primary, context.spec.tone) {
            (true, Tone::Danger) => (p.danger, p.on_danger),
            (true, Tone::Normal) => (p.accent, p.on_accent),
            (false, _) => (p.footer, p.text),
        };
        let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
        let focused = item.itemState.0 & ODS_FOCUS.0 != 0;
        let fill = if pressed {
            mix(fill, ink, 24)
        } else if context.hovered == Some(id) {
            mix(fill, ink, 12)
        } else {
            fill
        };
        // SAFETY: the DC and rectangle are valid for this WM_DRAWITEM; the brushes
        // made here are deleted here.
        unsafe {
            let dc = item.hDC;
            let brush = CreateSolidBrush(colorref(fill));
            FillRect(dc, &raw const item.rcItem, brush);
            let _ = DeleteObject(HGDIOBJ::from(brush));
            if !primary {
                FrameRect(dc, &raw const item.rcItem, context.border);
            }
            if focused {
                // A second, inner edge in the text color: focus does not rest on
                // color alone, and it shows on the filled button too.
                let ring = CreateSolidBrush(colorref(ink));
                let mut edge = item.rcItem;
                edge.left += 2;
                edge.top += 2;
                edge.right -= 2;
                edge.bottom -= 2;
                FrameRect(dc, &raw const edge, ring);
                let _ = DeleteObject(HGDIOBJ::from(ring));
            }
            let mut buffer = [0u16; 128];
            let length = GetWindowTextW(item.hwndItem, &mut buffer).max(0) as usize;
            let old = SelectObject(dc, HGDIOBJ::from(context.font));
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, colorref(ink));
            let mut rect = item.rcItem;
            let mut flags = DT_CENTER | DT_VCENTER | DT_SINGLELINE;
            if item.itemState.0 & ODS_NOACCEL.0 != 0 {
                flags |= DT_HIDEPREFIX;
            }
            DrawTextW(dc, &mut buffer[..length], &raw mut rect, flags);
            SelectObject(dc, old);
        }
    }

    /// Buttons notice the pointer, so they can show it.
    unsafe extern "system" fn button_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: the parent is the dialog that subclassed this button.
        let parent = unsafe { GetParent(hwnd) }.ok();
        let context = parent.and_then(context_of);
        let Some(context) = context else {
            return LRESULT(0);
        };
        // SAFETY: GetDlgCtrlID only reads this live window.
        let id = unsafe { windows::Win32::UI::WindowsAndMessaging::GetDlgCtrlID(hwnd) } as u16;
        let hover = match msg {
            WM_MOUSEMOVE if context.hovered != Some(id) => {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                // SAFETY: `track` is sized and lives through the call.
                let _ = unsafe { TrackMouseEvent(&raw mut track) };
                Some(Some(id))
            }
            WM_MOUSELEAVE => Some(None),
            _ => None,
        };
        if let Some(hovered) = hover {
            context.hovered = hovered;
            // SAFETY: repainting this live control.
            let _ =
                unsafe { windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, false) };
        }
        // SAFETY: `button_proc` was BUTTON's window procedure, saved at subclassing;
        // a procedure pointer and an isize are the same size here.
        unsafe {
            let previous = std::mem::transmute::<
                isize,
                windows::Win32::UI::WindowsAndMessaging::WNDPROC,
            >(context.button_proc);
            CallWindowProcW(previous, hwnd, msg, wparam, lparam)
        }
    }

    /// The dialog's window procedure.
    unsafe extern "system" fn dialog_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> isize {
        if msg == WM_INITDIALOG {
            // SAFETY: `show` passed a pointer to its context, which outlives this.
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
            }
            if let Some(context) = context_of(hwnd) {
                set_up(hwnd, context);
            }
            return 0; // the focus has been set
        }
        let Some(context) = context_of(hwnd) else {
            return 0;
        };
        let hdc = HDC(wparam.0 as *mut c_void);
        match msg {
            WM_CTLCOLORDLG => context.surface.0 as isize,
            WM_CTLCOLORSTATIC => {
                // SAFETY: the DC is valid for this message.
                unsafe {
                    SetBkMode(hdc, TRANSPARENT);
                    SetTextColor(hdc, colorref(context.palette.text));
                }
                context.surface.0 as isize
            }
            WM_CTLCOLOREDIT => {
                // SAFETY: the DC is valid for this message.
                unsafe {
                    windows::Win32::Graphics::Gdi::SetBkColor(hdc, colorref(context.palette.field));
                    SetTextColor(hdc, colorref(context.palette.text));
                }
                context.field.0 as isize
            }
            WM_PAINT => {
                paint(hwnd, context);
                1
            }
            WM_DRAWITEM => {
                // SAFETY: for WM_DRAWITEM, lparam points at a DRAWITEMSTRUCT.
                if let Some(item) = unsafe { (lparam.0 as *const DRAWITEMSTRUCT).as_ref() } {
                    draw_button(item, context);
                }
                1
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as u16;
                if ![ID_OK, ID_CANCEL, ID_YES, ID_NO].contains(&id) {
                    return 0;
                }
                // SAFETY: GetFocus and GetDlgCtrlID only read live windows.
                let focused = unsafe {
                    let focus = GetFocus();
                    if focus.is_invalid() {
                        None
                    } else {
                        Some(windows::Win32::UI::WindowsAndMessaging::GetDlgCtrlID(focus) as u16)
                    }
                }
                .filter(|&id| id == context.confirm_id || id == context.cancel_id);
                let answer = answer_for(id, lparam.0 != 0, focused);
                if answer == context.confirm_id
                    && let Ok(field) = unsafe { GetDlgItem(Some(hwnd), i32::from(ID_FIELD)) }
                {
                    let mut buffer = [0u16; 512];
                    // SAFETY: the buffer is sized by the slice passed.
                    let length = unsafe { GetWindowTextW(field, &mut buffer) }.max(0) as usize;
                    context.text = Some(String::from_utf16_lossy(&buffer[..length]));
                }
                // SAFETY: ending this live dialog.
                let _ = unsafe { EndDialog(hwnd, answer as isize) };
                1
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color) -> f64 {
        let channel = |v: u8| {
            let s = f64::from(v) / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
    }

    fn contrast(a: Color, b: Color) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    fn presets() -> Vec<(&'static str, Theme)> {
        crate::theme::PRESETS
            .iter()
            .chain(&["copper-dark", "copper-light"])
            .map(|name| (*name, Theme::preset(name).expect("a preset")))
            .collect()
    }

    #[test]
    fn button_text_is_readable_on_any_color() {
        for (r, g, b) in [
            (0, 0, 0),
            (255, 255, 255),
            (128, 128, 128),
            (224, 164, 119),
            (180, 40, 40),
        ] {
            let background = Color::rgb(r, g, b);
            let text = readable_on(background);
            assert!(contrast(text, background) >= 4.5, "{background:?}");
        }
    }

    #[test]
    fn every_theme_gets_a_readable_dialog() {
        // Break caught: a skin nobody can read in one of the themes.
        for (name, theme) in presets() {
            let p = palette(&theme);
            assert!(contrast(p.text, p.surface) >= 4.5, "{name}: text");
            assert!(contrast(p.text, p.footer) >= 4.5, "{name}: quiet button");
            assert!(contrast(p.text, p.field) >= 4.5, "{name}: field");
            assert!(contrast(p.on_accent, p.accent) >= 4.5, "{name}: confirm");
            assert!(contrast(p.on_danger, p.danger) >= 4.5, "{name}: danger");
            assert!(
                contrast(p.accent, p.footer) >= 2.0,
                "{name}: confirm stands out"
            );
        }
    }

    #[test]
    fn a_light_theme_gets_a_light_title_bar() {
        assert!(palette(&Theme::copper_dark()).dark);
        assert!(!palette(&Theme::copper_light()).dark);
    }

    fn inside((x, y, w, h): Rect, (cw, ch): (i32, i32)) -> bool {
        x >= 0 && y >= 0 && w > 0 && h > 0 && x + w <= cw && y + h <= ch
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
    }

    fn measured(dpi: u32) -> Measured {
        Measured {
            dpi,
            heading_height: 20,
            message_height: 60,
            has_field: true,
            confirm_label: 40,
            cancel_label: Some(50),
            max_height: 4000,
        }
    }

    #[test]
    fn nothing_overlaps_or_leaves_the_dialog_at_any_scale() {
        for dpi in [96, 120, 144, 192, 288] {
            let l = layout(&measured(dpi));
            let parts = [
                l.icon,
                l.heading.expect("heading"),
                l.message,
                l.field.expect("field"),
                l.confirm,
                l.cancel.expect("cancel"),
            ];
            for (i, a) in parts.iter().enumerate() {
                assert!(inside(*a, l.client), "{dpi}: {a:?} in {:?}", l.client);
                for b in &parts[i + 1..] {
                    assert!(!overlap(*a, *b), "{dpi}: {a:?} and {b:?}");
                }
            }
            assert!(l.field.expect("field").1 + l.field.expect("field").3 <= l.footer_top);
            assert!(l.confirm.1 >= l.footer_top);
        }
    }

    #[test]
    fn buttons_are_big_enough_and_apart() {
        // 28 is a desktop control's comfortable height; 12 the gap between them.
        for dpi in [96, 144, 192] {
            let scale = |dips: i32| dips * dpi as i32 / 96;
            let l = layout(&measured(dpi));
            let (confirm, cancel) = (l.confirm, l.cancel.expect("cancel"));
            assert!(confirm.3 >= scale(28), "{dpi}: height {}", confirm.3);
            assert!(
                cancel.0 - (confirm.0 + confirm.2) >= scale(12),
                "{dpi}: gap"
            );
            assert_eq!(
                cancel.0 + cancel.2,
                l.client.0 - scale(16),
                "{dpi}: right edge"
            );
        }
    }

    #[test]
    fn a_long_label_gets_a_wide_button() {
        let mut m = measured(96);
        m.confirm_label = 300;
        assert!(layout(&m).confirm.2 > 300);
    }

    #[test]
    fn a_plain_message_needs_no_field_or_cancel() {
        let mut m = measured(96);
        m.has_field = false;
        m.heading_height = 0;
        m.cancel_label = None;
        let l = layout(&m);
        assert_eq!(l.heading, None);
        assert_eq!(l.field, None);
        assert_eq!(l.cancel, None);
    }

    #[test]
    fn a_huge_message_cannot_push_the_buttons_off_the_screen() {
        // Break caught: a long error leaving no way to press anything.
        let mut m = measured(96);
        m.message_height = 90_000;
        m.max_height = 700;
        let l = layout(&m);
        assert!(l.client.1 <= 700, "{:?}", l.client);
        assert!(inside(l.confirm, l.client));
        assert!(l.message.3 > 0);
    }

    #[test]
    fn a_short_message_still_clears_the_icon() {
        let mut m = measured(96);
        m.heading_height = 0;
        m.has_field = false;
        m.message_height = 16;
        let l = layout(&m);
        assert!(l.footer_top >= l.icon.1 + l.icon.3);
    }

    #[test]
    fn the_message_wraps_inside_the_margins() {
        assert!(text_width(96) > 200);
        assert!(text_width(96) < WIDTH);
        assert_eq!(text_width(192), text_width(96) * 2);
    }

    #[test]
    fn enter_means_the_button_you_are_on() {
        // Break caught: Tab to Yes, press Enter, and it answers No, or worse,
        // the other way round on a destructive question.
        assert_eq!(answer_for(ID_NO, true, Some(ID_YES)), ID_YES);
        assert_eq!(answer_for(ID_YES, true, Some(ID_NO)), ID_NO);
        assert_eq!(answer_for(ID_OK, true, Some(ID_CANCEL)), ID_CANCEL);
        assert_eq!(answer_for(ID_NO, true, Some(ID_NO)), ID_NO);
        // In a text field, Enter means the default button.
        assert_eq!(answer_for(ID_OK, true, None), ID_OK);
    }

    #[test]
    fn esc_and_the_close_box_always_cancel() {
        assert_eq!(answer_for(ID_CANCEL, false, Some(ID_YES)), ID_CANCEL);
        assert_eq!(answer_for(ID_CANCEL, true, Some(ID_OK)), ID_CANCEL);
    }

    #[test]
    fn a_program_choosing_a_button_gets_that_button() {
        // A message with no control behind it names the button itself.
        assert_eq!(answer_for(ID_YES, false, Some(ID_NO)), ID_YES);
    }
}
