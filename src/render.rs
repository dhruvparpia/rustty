//! Direct2D/DirectWrite renderer: paints a grid through an
//! `ID2D1HwndRenderTarget` (spec WIN-002, WIN-003).
//!
//! This module is an FFI boundary (NFR-401): all Direct2D/DirectWrite calls
//! are `unsafe` Win32 COM invocations. The renderer never panics on
//! Direct2D failures (NFR-216) — `paint` logs and returns.

use std::collections::HashMap;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_WINDOW_STATE_OCCLUDED,
    D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_TEXT_METRICS, DWriteCreateFactory, IDWriteFactory,
    IDWriteFontCollection, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::PCWSTR;
use windows_numerics::Vector2;

use crate::app::App;
use crate::color::Color;
use crate::grid::{Cell, CellAttribute, Grid};
use crate::pane::Pane;
use crate::tabbar::{self, TAB_BAR_HEIGHT, TAB_CHROME};
use crate::theme::Theme;

/// Font families tried in order (WIN-003): Cascadia first, Consolas last.
const FONT_FAMILIES: [&str; 3] = ["Cascadia Mono", "Cascadia Code", "Consolas"];

/// Default face size in points.
const FONT_POINTS: f32 = 11.0;

/// Extra line leading as a fraction of the measured glyph height.
const LINE_LEADING: f32 = 1.2;

/// Cell geometry derived from the font, in DIPs.
#[derive(Debug, Clone, Copy)]
pub struct CellMetrics {
    /// Cell width (monospace advance of `M`, rounded up).
    pub cell_width: f32,
    /// Cell height (glyph box with leading, rounded up).
    pub cell_height: f32,
    /// Drawn text-box height (used to vertically center glyphs).
    pub text_height: f32,
}

/// Device-independent pixels to physical pixels at `dpi`.
#[must_use]
pub fn dips_to_px(dips: f32, dpi: f32) -> u32 {
    (dips * dpi / 96.0).ceil() as u32
}

/// Physical pixels to device-independent pixels at `dpi`.
#[must_use]
pub fn px_to_dips(px: u32, dpi: f32) -> f32 {
    px as f32 * 96.0 / dpi
}

/// UTF-16 encode a string for DirectWrite layouts (no trailing NUL:
/// the bindings take the slice length as the string length).
fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// NUL-terminated UTF-16 for `PCWSTR` parameters.
fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn color_f(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r as f32 / 255.0,
        g: c.g as f32 / 255.0,
        b: c.b as f32 / 255.0,
        a: 1.0,
    }
}

fn color_key(c: Color) -> u32 {
    (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
}

/// Blend two colors: `t` is the fraction of `to` (0 = `from`, 1 = `to`).
/// Used for dimmed tab-bar labels on the predawn palette.
fn mix(from: Color, to: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let blend = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color {
        r: blend(from.r, to.r),
        g: blend(from.g, to.g),
        b: blend(from.b, to.b),
    }
}

/// Measure cell geometry for the chosen monospace font at `dpi`.
///
/// # Errors
///
/// Fails when the DirectWrite factory cannot be created.
pub fn measure_cells(dpi: u32) -> anyhow::Result<CellMetrics> {
    let dpi = dpi.max(96) as f32;
    let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
    let (_family, _formats, metrics) = build_font_resources(&dwrite, dpi)?;
    Ok(metrics)
}

/// Build the text formats for plain/bold/italic/bold-italic plus the cell
/// metrics, picking the first installed family from [`FONT_FAMILIES`].
fn build_font_resources(
    dwrite: &IDWriteFactory,
    dpi: f32,
) -> anyhow::Result<(String, [IDWriteTextFormat; 4], CellMetrics)> {
    let font_size = FONT_POINTS * dpi / 72.0; // points -> DIPs
    let mut collection: Option<IDWriteFontCollection> = None;
    unsafe { dwrite.GetSystemFontCollection(&raw mut collection, false) }?;
    let collection = collection.ok_or_else(|| anyhow::anyhow!("rusttyterm: no system fonts"))?;
    let mut chosen = FONT_FAMILIES[FONT_FAMILIES.len() - 1];
    for family in FONT_FAMILIES {
        let name = wstr(family);
        let mut index = 0u32;
        let mut exists = windows::core::BOOL(0);
        if unsafe {
            collection.FindFamilyName(PCWSTR::from_raw(name.as_ptr()), &mut index, &mut exists)
        }
        .is_ok()
            && exists.0 != 0
        {
            chosen = family;
            break;
        }
    }
    let formats: [IDWriteTextFormat; 4] = [
        make_format(
            dwrite,
            chosen,
            font_size,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
        )?,
        make_format(
            dwrite,
            chosen,
            font_size,
            DWRITE_FONT_WEIGHT_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
        )?,
        make_format(
            dwrite,
            chosen,
            font_size,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_ITALIC,
        )?,
        make_format(
            dwrite,
            chosen,
            font_size,
            DWRITE_FONT_WEIGHT_BOLD,
            DWRITE_FONT_STYLE_ITALIC,
        )?,
    ];
    let metrics = measure_cells_with(dwrite, &formats[0])?;
    Ok((chosen.to_string(), formats, metrics))
}

#[allow(
    clippy::too_many_arguments,
    reason = "DirectWrite CreateTextFormat arity"
)]
fn make_format(
    dwrite: &IDWriteFactory,
    family: &str,
    size: f32,
    weight: windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT,
    style: windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STYLE,
) -> anyhow::Result<IDWriteTextFormat> {
    let name = wstr(family);
    let locale = wstr("");
    let format = unsafe {
        dwrite.CreateTextFormat(
            PCWSTR::from_raw(name.as_ptr()),
            None,
            weight,
            style,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            PCWSTR::from_raw(locale.as_ptr()),
        )
    }?;
    Ok(format)
}

fn measure_cells_with(
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
) -> anyhow::Result<CellMetrics> {
    let width = layout_height(dwrite, format, "M")?.width;
    let glyph = layout_height(dwrite, format, "Mg")?;
    let cell_width = width.ceil().max(1.0);
    let cell_height = (glyph.height * LINE_LEADING).ceil().max(1.0);
    Ok(CellMetrics {
        cell_width,
        cell_height,
        text_height: glyph.height,
    })
}

fn layout_height(
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    sample: &str,
) -> anyhow::Result<DWRITE_TEXT_METRICS> {
    let layout: IDWriteTextLayout =
        unsafe { dwrite.CreateTextLayout(&utf16(sample), format, 1000.0, 1000.0) }?;
    let mut metrics = DWRITE_TEXT_METRICS::default();
    unsafe { layout.GetMetrics(&mut metrics) }?;
    Ok(metrics)
}

/// A Direct2D render target bound to a terminal window.
pub struct Renderer {
    target: ID2D1HwndRenderTarget,
    formats: [IDWriteTextFormat; 4],
    metrics: CellMetrics,
    dpi: f32,
    brushes: HashMap<u32, ID2D1SolidColorBrush>,
}

impl Renderer {
    /// Create the render target for `hwnd`, sized for `cols` x `rows` cells.
    ///
    /// # Errors
    ///
    /// Fails when the Direct2D/DirectWrite factories or the render target
    /// cannot be created.
    pub fn new(hwnd: HWND, dpi: u32, cols: usize, rows: usize) -> anyhow::Result<Self> {
        let dpi = dpi.max(96) as f32;
        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }?;
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        // Family name is diagnostics-only; the formats carry the chosen face.
        let (_family, formats, metrics) = build_font_resources(&dwrite, dpi)?;
        let width = dips_to_px(cols.max(1) as f32 * metrics.cell_width, dpi).max(1);
        let height = dips_to_px(
            rows.max(1) as f32 * metrics.cell_height + TAB_BAR_HEIGHT,
            dpi,
        )
        .max(1);
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: dpi,
            dpiY: dpi,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: D2D_SIZE_U { width, height },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let target = unsafe { factory.CreateHwndRenderTarget(&props, &hwnd_props) }?;
        Ok(Self {
            target,
            formats,
            metrics,
            dpi,
            brushes: HashMap::new(),
        })
    }

    /// Cell geometry, in DIPs.
    #[must_use]
    pub fn metrics(&self) -> CellMetrics {
        self.metrics
    }

    /// Resize the render target to a new client area in physical pixels.
    ///
    /// Direct2D errors are ignored: the next `WM_PAINT` retries.
    pub fn resize(&mut self, width_px: u32, height_px: u32) {
        let size = D2D_SIZE_U {
            width: width_px.max(1),
            height: height_px.max(1),
        };
        let res = unsafe { self.target.Resize(&size) };
        if res.is_err() {
            eprintln!("rusttyterm: Direct2D resize failed");
        }
    }

    /// Grid size that fits the current client area (the tab bar strip is
    /// carved off the top).
    #[must_use]
    pub fn grid_size(&self) -> (usize, usize) {
        let px = unsafe { self.target.GetPixelSize() };
        let w = px_to_dips(px.width, self.dpi);
        let h = (px_to_dips(px.height, self.dpi) - TAB_BAR_HEIGHT).max(0.0);
        (
            (w / self.metrics.cell_width).floor().max(1.0) as usize,
            (h / self.metrics.cell_height).floor().max(1.0) as usize,
        )
    }

    /// Paint the app: tab bar strip, active pane viewport, cursor.
    /// Never panics on Direct2D failure (NFR-216).
    pub fn paint(&mut self, app: &App, theme: &Theme) {
        if unsafe { self.target.CheckWindowState() } == D2D1_WINDOW_STATE_OCCLUDED {
            return;
        }
        let px = unsafe { self.target.GetPixelSize() };
        let width = px_to_dips(px.width, self.dpi);
        let height = px_to_dips(px.height, self.dpi);
        unsafe { self.target.BeginDraw() };
        self.fill_rect(0.0, 0.0, width, height, theme.background);
        unsafe { self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED) };
        if let Some(tab) = app.active_tab() {
            let pane = tab.pane();
            let grid = pane.grid();
            let rows = grid.rows();
            let cols = (grid.cols()).min((width / self.metrics.cell_width).floor() as usize);
            for y in 0..rows {
                let Some(line) = grid.row(y) else { continue };
                let n = line.cells.len().min(cols);
                let cells = &line.cells[..n];
                self.paint_row_backgrounds(y, cells, theme);
                self.paint_row_selection(y, grid.abs_of_view(y), n, pane.selection(), theme);
                self.paint_row_text(y, cells, theme);
                self.paint_row_decorations(y, cells, theme);
            }
            self.paint_cursor(pane, grid, theme);
        }
        self.paint_tab_bar(app, theme, width);
        unsafe {
            self.target
                .SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE)
        };
        // EndDraw flushes the frame; a failure here drops the frame only
        // (NFR-216): log it and let the next paint retry.
        if unsafe { self.target.EndDraw(None, None) }.is_err() {
            eprintln!("rusttyterm: Direct2D EndDraw failed");
        }
    }

    /// Copy the printable text of the active viewport (Ctrl+Shift+C MVP
    /// copy: whole visible screen, trailing blanks trimmed).
    #[must_use]
    pub fn visible_text(grid: &Grid) -> String {
        let mut out = String::new();
        for y in 0..grid.rows() {
            if let Some(line) = grid.row(y) {
                let text: String = line.cells.iter().map(|c| c.char).collect();
                out.push_str(text.trim_end());
                out.push('\n');
            }
        }
        while out.ends_with('\n') {
            out.pop();
        }
        out
    }

    /// Fill the mouse-selection span on viewport row `y` (drawn between
    /// the row background and the row text, so glyphs stay legible).
    /// `abs` is the row's absolute line id; `n_cols` is the painted width.
    fn paint_row_selection(
        &mut self,
        y: usize,
        abs: Option<usize>,
        n_cols: usize,
        sel: Option<crate::selection::Selection>,
        theme: &Theme,
    ) {
        let Some((lo, hi)) = sel.and_then(|s| abs.and_then(|a| s.cols_on(a, n_cols))) else {
            return;
        };
        if hi <= lo {
            return;
        }
        let top = TAB_BAR_HEIGHT + y as f32 * self.metrics.cell_height;
        self.fill_rect(
            lo as f32 * self.metrics.cell_width,
            top,
            hi as f32 * self.metrics.cell_width,
            top + self.metrics.cell_height,
            theme.selection,
        );
    }

    /// Paint the tab bar strip: bar background, per-tab titles (active
    /// tab highlighted), and the `+` new-tab button (tab bar P0).
    fn paint_tab_bar(&mut self, app: &App, theme: &Theme, width: f32) {
        self.fill_rect(0.0, 0.0, width, TAB_BAR_HEIGHT, theme.tab_bar);
        let titles: Vec<String> = app.tabs().iter().map(|t| t.title().to_string()).collect();
        let layout = tabbar::layout(&titles, width, self.metrics.cell_width);
        let active = app.active_index();
        let y_top = (TAB_BAR_HEIGHT - self.metrics.text_height) / 2.0;
        for (i, &(lo, hi)) in layout.tabs.iter().enumerate() {
            if hi <= lo {
                continue;
            }
            if i == active {
                self.fill_rect(lo, 0.0, hi, TAB_BAR_HEIGHT, theme.tab_active);
            }
            let label =
                tabbar::fit_title(&titles[i], (hi - lo) - TAB_CHROME, self.metrics.cell_width);
            if label.is_empty() {
                continue;
            }
            let units = utf16(&label);
            let rect = D2D_RECT_F {
                left: lo + TAB_CHROME / 2.0,
                top: y_top,
                right: hi - TAB_CHROME / 2.0,
                bottom: y_top + self.metrics.text_height,
            };
            let color = if i == active {
                theme.foreground
            } else {
                mix(theme.foreground, theme.tab_bar, 0.45)
            };
            let Some(brush) = self.brush(color) else {
                continue;
            };
            let format = &self.formats[0];
            unsafe {
                self.target.DrawText(
                    &units,
                    format,
                    &rect,
                    &brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        }
        let (lo, hi) = layout.plus;
        if hi > lo {
            let cx = (lo + hi) / 2.0;
            let units = utf16("+");
            let rect = D2D_RECT_F {
                left: cx - self.metrics.cell_width / 2.0,
                top: y_top,
                right: cx + self.metrics.cell_width / 2.0,
                bottom: y_top + self.metrics.text_height,
            };
            let color = mix(theme.foreground, theme.tab_bar, 0.35);
            if let Some(brush) = self.brush(color) {
                let format = &self.formats[0];
                unsafe {
                    self.target.DrawText(
                        &units,
                        format,
                        &rect,
                        &brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }
        }
    }

    fn effective_colors(cell: &Cell, theme: &Theme) -> (Color, Color) {
        let fg = cell.fg.unwrap_or(theme.foreground);
        let bg = cell.bg.unwrap_or(theme.background);
        if cell.attributes.contains(CellAttribute::INVERSE) {
            (bg, fg)
        } else {
            (fg, bg)
        }
    }

    fn paint_row_backgrounds(&mut self, y: usize, cells: &[Cell], theme: &Theme) {
        let mut x = 0;
        while x < cells.len() {
            let (_, bg) = Self::effective_colors(&cells[x], theme);
            if bg == theme.background {
                x += 1;
                continue;
            }
            let mut end = x + 1;
            while end < cells.len() && Self::effective_colors(&cells[end], theme).1 == bg {
                end += 1;
            }
            let top = TAB_BAR_HEIGHT + y as f32 * self.metrics.cell_height;
            self.fill_rect(
                x as f32 * self.metrics.cell_width,
                top,
                end as f32 * self.metrics.cell_width,
                top + self.metrics.cell_height,
                bg,
            );
            x = end;
        }
    }

    fn paint_row_text(&mut self, y: usize, cells: &[Cell], theme: &Theme) {
        let cw = self.metrics.cell_width;
        let ch = self.metrics.cell_height;
        let y_top = TAB_BAR_HEIGHT + y as f32 * ch + (ch - self.metrics.text_height) / 2.0;
        let mut i = 0;
        while i < cells.len() {
            let cell = &cells[i];
            if cell.char == ' ' || cell.attributes.contains(CellAttribute::HIDDEN) {
                i += 1;
                continue;
            }
            let attrs = cell.attributes;
            let (fg, _) = Self::effective_colors(cell, theme);
            let mut end = i + 1;
            while end < cells.len() {
                let next = &cells[end];
                if next.char == ' '
                    || next.attributes.contains(CellAttribute::HIDDEN)
                    || next.attributes != attrs
                    || Self::effective_colors(next, theme).0 != fg
                {
                    break;
                }
                end += 1;
            }
            let text: String = cells[i..end].iter().map(|c| c.char).collect();
            let units = utf16(&text);
            let rect = D2D_RECT_F {
                left: i as f32 * cw,
                top: y_top,
                right: end as f32 * cw,
                bottom: y_top + self.metrics.text_height,
            };
            let Some(brush) = self.brush(fg) else {
                i = end;
                continue;
            };
            let format = &self.formats[format_index(attrs)];
            unsafe {
                self.target.DrawText(
                    &units,
                    format,
                    &rect,
                    &brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
            i = end;
        }
    }

    fn paint_row_decorations(&mut self, y: usize, cells: &[Cell], theme: &Theme) {
        let cw = self.metrics.cell_width;
        let ch = self.metrics.cell_height;
        for (x, cell) in cells.iter().enumerate() {
            if cell.char == ' ' || cell.attributes.contains(CellAttribute::HIDDEN) {
                continue;
            }
            let (fg, _) = Self::effective_colors(cell, theme);
            let left = x as f32 * cw;
            let right = left + cw;
            if cell.attributes.contains(CellAttribute::UNDERLINE) {
                let yline = TAB_BAR_HEIGHT + y as f32 * ch + ch - 1.0;
                self.stroke_line(left, yline, right, yline, fg);
            }
            if cell.attributes.contains(CellAttribute::STRIKE) {
                let yline = TAB_BAR_HEIGHT + y as f32 * ch + ch / 2.0;
                self.stroke_line(left, yline, right, yline, fg);
            }
        }
    }

    fn paint_cursor(&mut self, pane: &Pane, grid: &Grid, theme: &Theme) {
        if grid.scroll_offset() != 0 || !pane.cursor_visible() {
            return;
        }
        let row = pane.parser().cursor_row;
        let col = pane.parser().cursor_col;
        if row >= grid.rows() || col >= grid.cols() {
            return;
        }
        let top = TAB_BAR_HEIGHT + row as f32 * self.metrics.cell_height;
        let left = col as f32 * self.metrics.cell_width;
        self.fill_rect(
            left,
            top,
            left + self.metrics.cell_width,
            top + self.metrics.cell_height,
            theme.cursor,
        );
    }

    fn fill_rect(&mut self, left: f32, top: f32, right: f32, bottom: f32, color: Color) {
        let rect = D2D_RECT_F {
            left,
            top,
            right,
            bottom,
        };
        let Some(brush) = self.brush(color) else {
            return;
        };
        unsafe { self.target.FillRectangle(&rect, &brush) };
    }

    fn stroke_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: Color) {
        let Some(brush) = self.brush(color) else {
            return;
        };
        unsafe {
            self.target.DrawLine(
                Vector2 { X: x0, Y: y0 },
                Vector2 { X: x1, Y: y1 },
                &brush,
                1.0,
                None,
            );
        }
    }

    fn brush(&mut self, color: Color) -> Option<ID2D1SolidColorBrush> {
        if let Some(brush) = self.brushes.get(&color_key(color)) {
            return Some(brush.clone());
        }
        let created = unsafe { self.target.CreateSolidColorBrush(&color_f(color), None) };
        match created {
            Ok(brush) => {
                self.brushes.insert(color_key(color), brush.clone());
                Some(brush)
            }
            Err(_) => {
                // Brush failure must not stop the frame (NFR-216): the
                // affected rectangles/text are simply skipped.
                eprintln!("rusttyterm: Direct2D brush creation failed");
                None
            }
        }
    }
}

/// Map SGR attributes onto the cached text formats.
fn format_index(attrs: CellAttribute) -> usize {
    let bold = usize::from(attrs.contains(CellAttribute::BOLD));
    let italic = usize::from(attrs.contains(CellAttribute::ITALIC));
    bold * 2 + italic
}
