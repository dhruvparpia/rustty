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
use crate::config::FontConfig;
use crate::grid::{Cell, CellAttribute, Grid};
use crate::pane::Pane;
use crate::tabbar::{self, TAB_CHROME};
use crate::theme::Theme;

/// Font families used as the fallback chain (WIN-003/FONT-018): Cascadia
/// first, Consolas last. The configured family (FONT-002) is tried before
/// this chain.
const FONT_FAMILIES: [&str; 3] = ["Cascadia Mono", "Cascadia Code", "Consolas"];

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

/// Measure cell geometry for the configured font at `dpi` (FONT-002/011).
///
/// # Errors
///
/// Fails when the DirectWrite factory cannot be created.
pub fn measure_cells(dpi: u32, font: &FontConfig) -> anyhow::Result<CellMetrics> {
    let dpi = dpi.max(96) as f32;
    let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
    let (_family, _formats, metrics) = build_font_resources(&dwrite, dpi, font)?;
    Ok(metrics)
}

/// Build the text formats for plain/bold/italic/bold-italic plus the cell
/// metrics, picking the configured family when installed, else the first
/// installed family from [`FONT_FAMILIES`] (FONT-002/018).
fn build_font_resources(
    dwrite: &IDWriteFactory,
    dpi: f32,
    font: &FontConfig,
) -> anyhow::Result<(String, [IDWriteTextFormat; 4], CellMetrics)> {
    let font_size = font.size * dpi / 72.0; // points -> DIPs
    let mut collection: Option<IDWriteFontCollection> = None;
    unsafe { dwrite.GetSystemFontCollection(&raw mut collection, false) }?;
    let collection = collection.ok_or_else(|| anyhow::anyhow!("rusttyterm: no system fonts"))?;
    let mut chosen: &str = FONT_FAMILIES[FONT_FAMILIES.len() - 1];
    let configured = font.family.as_str();
    for family in std::iter::once(configured).chain(FONT_FAMILIES) {
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
    let metrics = measure_cells_with(dwrite, &formats[0], font.line_height)?;
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

/// Measure the cell box for `format`: width = advance of `M` (rounded up),
/// height = glyph box scaled by `line_height` (SPEC `line-height`).
fn measure_cells_with(
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    line_height: f32,
) -> anyhow::Result<CellMetrics> {
    let width = layout_height(dwrite, format, "M")?.width;
    let glyph = layout_height(dwrite, format, "Mg")?;
    let cell_width = width.ceil().max(1.0);
    let cell_height = (glyph.height * line_height).ceil().max(1.0);
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
    /// Tab-bar strip height in DIPs; 0.0 when the bar is hidden
    /// (`tab-bar = "auto"` with one tab, or `"hidden"`, TAB-010).
    tab_bar_height: f32,
    brushes: HashMap<u32, ID2D1SolidColorBrush>,
    /// Reused UTF-16 buffer: the text paint path fills this instead of
    /// building a `String` + `Vec<u16>` per run (NFR-113: no allocations in
    /// the render hot path).
    utf16: Vec<u16>,
    /// Reused tab-bar buffers: titles, layout, and the fitted label.
    titles: Vec<String>,
    layout: tabbar::Layout,
    label: String,
    /// Reused per-row damage mask for [`Renderer::paint_damage`].
    mask: Vec<bool>,
    /// Last painted frame, used to work out what a damage repaint must cover.
    damage: Damage,
}

/// What the last painted frame looked like: enough to decide which rows a
/// damage repaint has to cover (grid damage, cursor move, selection change).
#[derive(Debug, Default, Clone, Copy)]
struct Damage {
    /// Index of the tab whose pane was painted.
    active_tab: usize,
    /// Grid size the last frame was painted at.
    grid_cols: usize,
    grid_rows: usize,
    /// Row the cursor block was painted on last frame (`usize::MAX` when
    /// the block was not drawn).
    cursor_row: usize,
    /// Mouse selection painted last frame.
    selection: Option<crate::selection::Selection>,
    /// `false` until the first frame is on screen.
    painted: bool,
}

/// Cell the cursor block occupies, or `None` when it is not drawn
/// (scrolled back, or the shell hid it).
fn cursor_cell(pane: &Pane, grid: &Grid) -> Option<(usize, usize)> {
    if grid.scroll_offset() != 0 || !pane.cursor_visible() {
        return None;
    }
    let row = pane.parser().cursor_row;
    let col = pane.parser().cursor_col;
    if row >= grid.rows() || col >= grid.cols() {
        return None;
    }
    Some((row, col))
}

/// Viewport rows a selection covers, clamped to `rows`.
fn selection_rows(
    grid: &Grid,
    sel: crate::selection::Selection,
    rows: usize,
    out: &mut (usize, usize),
) -> bool {
    let Some(lo) = grid.view_of_abs(sel.start.row) else {
        return false;
    };
    let hi = grid
        .view_of_abs(sel.end.row)
        .unwrap_or(rows.saturating_sub(1))
        .min(rows.saturating_sub(1));
    *out = (lo.min(hi), hi);
    true
}

impl Renderer {
    /// Create the render target for `hwnd`, sized for `cols` x `rows` cells
    /// plus the tab-bar strip of `tab_bar_height` DIPs.
    ///
    /// # Errors
    ///
    /// Fails when the Direct2D/DirectWrite factories or the render target
    /// cannot be created.
    pub fn new(
        hwnd: HWND,
        dpi: u32,
        font: &FontConfig,
        cols: usize,
        rows: usize,
        tab_bar_height: f32,
    ) -> anyhow::Result<Self> {
        let dpi = dpi.max(96) as f32;
        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }?;
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        // Family name is diagnostics-only; the formats carry the chosen face.
        let (_family, formats, metrics) = build_font_resources(&dwrite, dpi, font)?;
        let width = dips_to_px(cols.max(1) as f32 * metrics.cell_width, dpi).max(1);
        let height = dips_to_px(
            rows.max(1) as f32 * metrics.cell_height + tab_bar_height,
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
            tab_bar_height,
            brushes: HashMap::new(),
            utf16: Vec::with_capacity(cols.max(1) * 2),
            titles: Vec::new(),
            layout: tabbar::Layout::default(),
            label: String::new(),
            mask: vec![true; rows.max(1)],
            damage: Damage::default(),
        })
    }

    /// Cell geometry, in DIPs.
    #[must_use]
    pub fn metrics(&self) -> CellMetrics {
        self.metrics
    }

    /// Tab-bar strip height in DIPs (0.0 when hidden).
    #[must_use]
    pub fn tab_bar_height(&self) -> f32 {
        self.tab_bar_height
    }

    /// Set the tab-bar strip height (TAB-010 visibility modes). The grid
    /// area below the strip changes with it — callers re-read
    /// [`Renderer::grid_size`] and resize the grid afterwards.
    pub fn set_tab_bar_height(&mut self, height: f32) {
        self.tab_bar_height = height.max(0.0);
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
        let h = (px_to_dips(px.height, self.dpi) - self.tab_bar_height).max(0.0);
        (
            (w / self.metrics.cell_width).floor().max(1.0) as usize,
            (h / self.metrics.cell_height).floor().max(1.0) as usize,
        )
    }

    /// Paint the whole client area: tab bar strip, active pane viewport,
    /// cursor. This is the full repaint, used for system-initiated
    /// `WM_PAINT` (expose, resize, coming back from occluded).
    /// Never panics on Direct2D failure (NFR-216).
    pub fn paint(&mut self, app: &App, theme: &Theme) {
        self.paint_frame(app, theme, true);
    }

    /// Paint only what changed since the last frame (NFR-113 damage-only
    /// redraw): the rows the grid wrote, plus the rows a cursor move or a
    /// selection change touched, plus the tab-bar strip. Falls back to a
    /// full repaint when the tab, the grid size, or the whole grid changed.
    ///
    /// The caller clears the grid damage flags afterwards
    /// (see [`crate::app::App::clear_grid_damage`]); a dropped frame keeps
    /// them set, so the next call repaints everything.
    pub fn paint_damage(&mut self, app: &App, theme: &Theme) {
        self.paint_frame(app, theme, false);
    }

    /// Rows the last frame painted, for the damage-only benchmark
    /// ([`crate::perf_bench`]). Not API: the row count of `self.mask`.
    #[doc(hidden)]
    pub fn last_mask_rows(&self) -> usize {
        self.mask.iter().filter(|&&row| row).count()
    }

    fn paint_frame(&mut self, app: &App, theme: &Theme, force_full: bool) {
        if unsafe { self.target.CheckWindowState() } == D2D1_WINDOW_STATE_OCCLUDED {
            return;
        }
        let px = unsafe { self.target.GetPixelSize() };
        let width = px_to_dips(px.width, self.dpi);
        let height = px_to_dips(px.height, self.dpi);
        let full = force_full
            || !self.damage.painted
            || app.active_tab().is_some_and(|tab| {
                let grid = tab.pane().grid();
                grid.all_dirty()
                    || self.damage.active_tab != app.active_index()
                    || self.damage.grid_cols != grid.cols()
                    || self.damage.grid_rows != grid.rows()
            });
        self.build_mask(app, full);
        unsafe { self.target.BeginDraw() };
        if full {
            self.fill_rect(0.0, 0.0, width, height, theme.background);
        }
        unsafe { self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED) };
        if let Some(tab) = app.active_tab() {
            let pane = tab.pane();
            let grid = pane.grid();
            let rows = grid.rows();
            let cols = (grid.cols()).min((width / self.metrics.cell_width).floor() as usize);
            for y in 0..rows {
                if !self.mask[y] {
                    continue;
                }
                let Some(line) = grid.row(y) else { continue };
                let n = line.cells.len().min(cols);
                let cells = &line.cells[..n];
                let top = self.tab_bar_height + y as f32 * self.metrics.cell_height;
                self.fill_rect(
                    0.0,
                    top,
                    width,
                    top + self.metrics.cell_height,
                    theme.background,
                );
                self.paint_row_backgrounds(y, cells, theme);
                self.paint_row_selection(y, grid.abs_of_view(y), n, pane.selection(), theme);
                self.paint_row_text(y, cells, theme);
                self.paint_row_decorations(y, cells, theme);
            }
            self.paint_cursor(pane, grid, theme);
        }
        if self.tab_bar_height > 0.0 {
            self.paint_tab_bar(app, theme, width);
        }
        unsafe {
            self.target
                .SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE)
        };
        // EndDraw flushes the frame; a failure here drops the frame only
        // (NFR-216): log it, keep the damage flags set, and let the next
        // paint start from a full repaint.
        if unsafe { self.target.EndDraw(None, None) }.is_err() {
            eprintln!("rusttyterm: Direct2D EndDraw failed");
            self.damage.painted = false;
            return;
        }
        if let Some(tab) = app.active_tab() {
            let pane = tab.pane();
            self.damage.active_tab = app.active_index();
            self.damage.grid_cols = pane.grid().cols();
            self.damage.grid_rows = pane.grid().rows();
            self.damage.cursor_row =
                cursor_cell(pane, pane.grid()).map_or(usize::MAX, |(row, _)| row);
            self.damage.selection = pane.selection();
        }
        self.damage.painted = true;
    }

    /// Fill the reused row mask with the viewport rows a damage repaint must
    /// cover: grid damage, the rows the cursor block moved between, and the
    /// rows the selection overlay changed on. Allocation-free after warmup.
    fn build_mask(&mut self, app: &App, full: bool) {
        let rows = app.active_tab().map_or(0, |tab| tab.pane().grid().rows());
        self.mask.clear();
        self.mask.resize(rows, false);
        if rows == 0 {
            return;
        }
        let Some(tab) = app.active_tab() else { return };
        let pane = tab.pane();
        let grid = pane.grid();
        let mark = |mask: &mut [bool], lo: usize, hi: usize| {
            let hi = hi.min(rows - 1);
            for row in &mut mask[lo.min(hi)..=hi] {
                *row = true;
            }
        };
        if full {
            mark(&mut self.mask, 0, rows - 1);
            return;
        }
        grid.for_each_dirty_row(|row| {
            if row < rows {
                self.mask[row] = true;
            }
        });
        // A moved block leaves a hole in the row it was painted on.
        let cursor = cursor_cell(pane, grid);
        let cursor_row = cursor.map(|(row, _)| row);
        if cursor_row != Some(self.damage.cursor_row) {
            if self.damage.cursor_row < rows {
                self.mask[self.damage.cursor_row] = true;
            }
            if let Some(row) = cursor_row {
                self.mask[row] = true;
            }
        }
        // The overlay has to be cleared where it was and drawn where it is.
        let selection = pane.selection();
        if selection != self.damage.selection {
            let mut span = (0usize, 0usize);
            for sel in [selection, self.damage.selection].into_iter().flatten() {
                if selection_rows(grid, sel, rows, &mut span) {
                    mark(&mut self.mask, span.0, span.1);
                }
            }
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
        let top = self.tab_bar_height + y as f32 * self.metrics.cell_height;
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
    /// Every buffer it needs is reused from the renderer, so a frame with
    /// the bar visible allocates nothing (NFR-113).
    fn paint_tab_bar(&mut self, app: &App, theme: &Theme, width: f32) {
        self.fill_rect(0.0, 0.0, width, self.tab_bar_height, theme.tab_bar);
        // Titles: fill the reused slots with the reused scratch String,
        // swapping buffers so each slot keeps its own capacity.
        let cw = self.metrics.cell_width;
        let mut label = std::mem::take(&mut self.label);
        let mut units = std::mem::take(&mut self.utf16);
        let titles = &mut self.titles;
        titles.resize(app.tabs().len(), String::new());
        for (slot, tab) in titles.iter_mut().zip(app.tabs()) {
            label.clear();
            label.push_str(tab.title());
            std::mem::swap(slot, &mut label);
        }
        tabbar::layout_into(
            &self.titles,
            width,
            self.metrics.cell_width,
            &mut self.layout,
        );
        let active = app.active_index();
        let y_top = (self.tab_bar_height - self.metrics.text_height) / 2.0;
        let plus = self.layout.plus;
        let tabs = std::mem::take(&mut self.layout.tabs);
        for (i, &(lo, hi)) in tabs.iter().enumerate() {
            if hi <= lo {
                continue;
            }
            if i == active {
                self.fill_rect(lo, 0.0, hi, self.tab_bar_height, theme.tab_active);
            }
            tabbar::fit_title_into(&self.titles[i], (hi - lo) - TAB_CHROME, cw, &mut label);
            if label.is_empty() {
                continue;
            }
            units.clear();
            units.extend(label.encode_utf16());
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
        self.layout.tabs = tabs;
        let (lo, hi) = plus;
        if hi > lo {
            let cx = (lo + hi) / 2.0;
            units.clear();
            units.push(u16::from(b'+'));
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
        units.clear();
        self.utf16 = units;
        self.label = label;
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
            let top = self.tab_bar_height + y as f32 * self.metrics.cell_height;
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

    /// Paint one row's glyphs by runs of identical style + colour. The
    /// UTF-16 text is built into the reused `self.utf16` buffer: the hot
    /// path allocates nothing per frame (NFR-113).
    fn paint_row_text(&mut self, y: usize, cells: &[Cell], theme: &Theme) {
        let cw = self.metrics.cell_width;
        let ch = self.metrics.cell_height;
        let y_top = self.tab_bar_height + y as f32 * ch + (ch - self.metrics.text_height) / 2.0;
        let mut units = std::mem::take(&mut self.utf16);
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
            units.clear();
            for ch in cells[i..end].iter().map(|cell| cell.char) {
                // `encode_utf16` writes into the stack buffer and reports how
                // many units it wrote: surrogates included, no allocation.
                let mut buf = [0u16; 2];
                let count = ch.encode_utf16(&mut buf).len();
                units.extend_from_slice(&buf[..count]);
            }
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
        units.clear();
        self.utf16 = units;
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
                let yline = self.tab_bar_height + y as f32 * ch + ch - 1.0;
                self.stroke_line(left, yline, right, yline, fg);
            }
            if cell.attributes.contains(CellAttribute::STRIKE) {
                let yline = self.tab_bar_height + y as f32 * ch + ch / 2.0;
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
        let top = self.tab_bar_height + row as f32 * self.metrics.cell_height;
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
