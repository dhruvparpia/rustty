//! Direct2D/DirectWrite renderer: paints a tab's pane tree through an
//! `ID2D1HwndRenderTarget` (spec WIN-002, WIN-003, SPLIT-002/014/015).
//!
//! A tab paints as its [`crate::split`] layout flattened: every leaf gets
//! its own pixel origin, pane borders (theme `split_border`/`split_focus`),
//! a title chip when the tab is split, and its cursor — solid on the
//! focused pane, hollow on the others. Damage tracking is per pane: the
//! row mask covers the whole tab (each leaf's viewport rows stacked in
//! layout order), so a damage repaint touches only rows some pane changed.
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
use crate::glyph_cache::{self, GlyphAtlas, GlyphKey, Tile};
use crate::grid::{Cell, CellAttribute, Grid};
use crate::pane::Pane;
use crate::selection::Selection;
use crate::split::Rect;
use crate::tab::Tab;
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

pub(crate) fn color_f(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r as f32 / 255.0,
        g: c.g as f32 / 255.0,
        b: c.b as f32 / 255.0,
        a: 1.0,
    }
}

/// Pack a color into the 24-bit key the brush and atlas caches use.
///
/// `const` so [`crate::glyph_cache::GlyphKey::new`] stays `const`.
pub(crate) const fn color_key(c: Color) -> u32 {
    // `as` casts: `u32::from` is not a const trait yet.
    (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32
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
    /// Reused row-damage mask for [`Renderer::paint_damage`]: one entry per
    /// tab row (each leaf's viewport rows stacked in layout order).
    mask: Vec<bool>,
    /// Last painted frame, used to work out what a damage repaint must cover.
    damage: Damage,
    /// Last painted frame's per-pane state (flatten order).
    pane_damage: Vec<PaneDamage>,
    /// `(pane_count, layout cols, layout rows)` of the last painted frame;
    /// any change forces a full repaint.
    shape: (usize, usize, usize),
    /// Glyph atlas (spec §20): created on the first paint, when the target
    /// exists. `None` = no atlas, glyphs go through the `DrawText` path.
    atlas: Option<GlyphAtlas>,
    /// Atlas creation failed: stop retrying per frame and stop logging.
    atlas_failed: bool,
}

/// What the last painted frame looked like: enough to decide which rows a
/// damage repaint has to cover (grid damage, cursor moves, selection
/// changes) plus the per-pane snapshot in [`PaneDamage`].
#[derive(Debug, Clone, Copy)]
struct Damage {
    /// Index of the tab whose panes were painted (`usize::MAX` before the
    /// first frame; a tab switch forces a full repaint).
    active_tab: usize,
    /// `false` until the first frame is on screen.
    painted: bool,
}

impl Default for Damage {
    fn default() -> Self {
        Self {
            active_tab: usize::MAX,
            painted: false,
        }
    }
}

/// Last painted state of one pane: the row-damage inputs tracked per pane
/// (grid size, cursor row, selection).
#[derive(Debug, Default, Clone, Copy)]
struct PaneDamage {
    /// Grid size the pane was painted at.
    grid_cols: usize,
    grid_rows: usize,
    /// Row the cursor block was painted on (`usize::MAX` when not drawn).
    cursor_row: usize,
    /// Mouse selection painted on this pane.
    selection: Option<Selection>,
}

/// Flatten index of the leaf whose layout box covers cell `(col, row)`
/// of the active tab, `None` when the cell is outside every pane.
fn leaf_at(tab: &Tab, col: usize, row: usize) -> Option<usize> {
    let mut found = None;
    let mut idx = 0usize;
    tab.for_each_pane_ref(&mut |_: &Pane, rect: Rect| {
        if rect.contains(col, row) {
            found = Some(idx);
        }
        idx += 1;
    });
    found
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
            mask: Vec::new(),
            damage: Damage::default(),
            pane_damage: Vec::new(),
            shape: (0, 0, 0),
            atlas: None,
            atlas_failed: false,
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
    /// Direct2D invalidates the resources created from a resized target, so
    /// the glyph atlas and the brush cache go with it; the next paint
    /// recreates them. Direct2D errors are ignored: the next `WM_PAINT`
    /// retries.
    pub fn resize(&mut self, width_px: u32, height_px: u32) {
        let size = D2D_SIZE_U {
            width: width_px.max(1),
            height: height_px.max(1),
        };
        let res = unsafe { self.target.Resize(&size) };
        if res.is_err() {
            eprintln!("rusttyterm: Direct2D resize failed");
        }
        self.atlas = None;
        self.atlas_failed = false;
        self.brushes.clear();
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

    /// Atlas counters for the benchmark ([`crate::perf_bench`]). Not API.
    #[doc(hidden)]
    #[must_use]
    pub fn atlas_stats(&self) -> glyph_cache::AtlasStats {
        self.atlas
            .as_ref()
            .map_or(glyph_cache::AtlasStats::default(), GlyphAtlas::stats)
    }

    /// Create the glyph atlas on the current render target (lazily, from the
    /// paint path: the target only exists once the window is up). A creation
    /// failure is permanent for this renderer: the frame paints through the
    /// `DrawText` path and the failure is logged once (NFR-216).
    ///
    /// `true` when an atlas is available.
    fn ensure_atlas(&mut self) -> bool {
        if self.atlas.is_some() {
            return true;
        }
        if self.atlas_failed {
            return false;
        }
        match GlyphAtlas::new(&self.target, &self.metrics, self.dpi) {
            Ok(atlas) => {
                self.atlas = Some(atlas);
                true
            }
            Err(e) => {
                self.atlas_failed = true;
                eprintln!("rusttyterm: glyph atlas unavailable: {e}");
                false
            }
        }
    }

    /// Collect pass for one pane: queue every glyph its masked rows will
    /// draw. [`crate::glyph_cache::GlyphAtlas::request`] deduplicates, so a
    /// frame of 80 x 24 identical cells queues one tile.
    fn collect_atlas_tiles(
        &mut self,
        atlas: &mut GlyphAtlas,
        pane: &Pane,
        rect: Rect,
        max_cols: usize,
        theme: &Theme,
    ) {
        let grid = pane.grid();
        let rows = rect.rows.min(grid.rows());
        let cols = rect.cols.min(max_cols.saturating_sub(rect.col));
        for y in 0..rows {
            if !self.mask.get(rect.row + y).copied().unwrap_or(false) {
                continue;
            }
            let Some(line) = grid.row(y) else { continue };
            let n = line.cells.len().min(cols);
            let span = Self::row_selection(pane, y, n);
            for (x, cell) in line.cells[..n].iter().enumerate() {
                if let Some(key) = Self::cell_key(cell, x, span, theme) {
                    let _ = atlas.request(&key);
                }
            }
        }
    }

    /// `true` when the active tab's panes cannot be damage-painted: tab
    /// switch, a changed split shape, a pane the renderer has not painted
    /// yet, a grid resize, or a grid flagged fully dirty.
    fn full_repaint_needed(&self, app: &App) -> bool {
        let Some(tab) = app.active_tab() else {
            return true;
        };
        if self.damage.active_tab != app.active_index() {
            return true;
        }
        if self.shape != (tab.pane_count(), tab.cols(), tab.rows()) {
            return true;
        }
        let mut full = false;
        let mut idx = 0usize;
        tab.for_each_pane_ref(&mut |pane: &Pane, _| {
            let grid = pane.grid();
            match self.pane_damage.get(idx) {
                None => full = true,
                Some(d) => {
                    if grid.all_dirty() || d.grid_cols != grid.cols() || d.grid_rows != grid.rows()
                    {
                        full = true;
                    }
                }
            }
            idx += 1;
        });
        full
    }

    /// Columns this frame may paint: the client width measured in cells.
    /// Each leaf clamps this further to its own box (SPLIT-002).
    fn frame_cols(&self, width: f32) -> usize {
        (width / self.metrics.cell_width).floor() as usize
    }

    fn paint_frame(&mut self, app: &App, theme: &Theme, force_full: bool) {
        if unsafe { self.target.CheckWindowState() } == D2D1_WINDOW_STATE_OCCLUDED {
            return;
        }
        let px = unsafe { self.target.GetPixelSize() };
        let width = px_to_dips(px.width, self.dpi);
        let height = px_to_dips(px.height, self.dpi);
        let full = force_full || !self.damage.painted || self.full_repaint_needed(app);
        self.build_mask(app, full);
        // Glyph atlas (spec §20): queue every glyph this frame wants, render
        // the new tiles into the atlas bitmap, then paint the frame with
        // blits. `render_pending` runs *before* the target's `BeginDraw`:
        // the atlas is its own draw session.
        let max_cols = self.frame_cols(width);
        let mut atlas = self.ensure_atlas().then(|| self.atlas.take()).flatten();
        if let Some(atlas) = atlas.as_mut() {
            if let Some(tab) = app.active_tab() {
                tab.for_each_pane_ref(&mut |pane: &Pane, rect: Rect| {
                    self.collect_atlas_tiles(atlas, pane, rect, max_cols, theme);
                });
            }
            atlas.render_pending(&self.formats, self.metrics.text_height);
        }
        unsafe { self.target.BeginDraw() };
        if full {
            self.fill_rect(0.0, 0.0, width, height, theme.background);
        }
        unsafe { self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED) };
        if let Some(tab) = app.active_tab() {
            let cw = self.metrics.cell_width;
            let ch = self.metrics.cell_height;
            tab.for_each_pane_ref(&mut |pane: &Pane, rect: Rect| {
                let left = rect.col as f32 * cw;
                let top0 = self.tab_bar_height + rect.row as f32 * ch;
                let grid = pane.grid();
                let rows = rect.rows.min(grid.rows());
                let cols = rect.cols.min(max_cols.saturating_sub(rect.col));
                for y in 0..rows {
                    if !self.mask[rect.row + y] {
                        continue;
                    }
                    let Some(line) = grid.row(y) else { continue };
                    let n = line.cells.len().min(cols);
                    let cells = &line.cells[..n];
                    let top = top0 + y as f32 * ch;
                    self.fill_rect(
                        left,
                        top,
                        left + rect.cols as f32 * cw,
                        top + ch,
                        theme.background,
                    );
                    let span = Self::row_selection(pane, y, n);
                    self.paint_row_backgrounds(left, top, cells, theme);
                    self.paint_row_selection(left, top, span, theme);
                    self.paint_row_text(left, top, cells, span, theme);
                    self.paint_row_decorations(left, top, cells, theme);
                }
            });
            if tab.pane_count() > 1 {
                self.paint_split_borders(tab, theme);
                self.paint_pane_titles(tab, theme);
            }
            let focused = tab.focused_index().unwrap_or(usize::MAX);
            let mut idx = 0usize;
            tab.for_each_pane_ref(&mut |pane: &Pane, rect: Rect| {
                self.paint_cursor(pane, rect, idx == focused, theme);
                idx += 1;
            });
        }
        self.atlas = atlas;
        if self.tab_bar_height > 0.0 {
            self.paint_tab_bar(app, theme, width);
        }
        unsafe {
            self.target
                .SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE)
        };
        // EndDraw flushes the frame; a failure here drops the frame only
        // (NFR-216): log it, keep the damage flags set, and let the next
        // paint start from a full repaint. The target's device-dependent
        // resources (atlas, brushes) go with it and are recreated.
        if unsafe { self.target.EndDraw(None, None) }.is_err() {
            eprintln!("rusttyterm: Direct2D EndDraw failed");
            self.damage.painted = false;
            self.pane_damage.clear();
            self.atlas = None;
            self.atlas_failed = false;
            self.brushes.clear();
            return;
        }
        self.commit_damage(app);
    }

    /// Fill the reused tab-row mask with the rows this frame must cover, per
    /// pane: grid damage, the rows each cursor block moved between, and the
    /// rows a selection overlay changed on. The mask has one entry per tab
    /// row; pane row `y` of leaf `rect` is mask index `rect.row + y`.
    /// Allocation-free after warmup.
    fn build_mask(&mut self, app: &App, full: bool) {
        let Some(tab) = app.active_tab() else {
            self.mask.clear();
            return;
        };
        let total = tab.rows();
        self.mask.clear();
        self.mask.resize(total, false);
        if total == 0 {
            return;
        }
        if full {
            self.mask.iter_mut().for_each(|m| *m = true);
            return;
        }
        // The snapshot is taken out so the closure can write rows into
        // `self.mask` while reading the previous cursor/selection state.
        let prev = std::mem::take(&mut self.pane_damage);
        let mut idx = 0usize;
        tab.for_each_pane_ref(&mut |pane: &Pane, rect: Rect| {
            let grid = pane.grid();
            let rows = rect.rows.min(grid.rows());
            grid.for_each_dirty_row(|row| {
                if row < rows {
                    self.mask[rect.row + row] = true;
                }
            });
            let d = prev.get(idx).copied().unwrap_or_default();
            // A moved block leaves a hole in the row it was painted on.
            let cursor_row = cursor_cell(pane, grid).map(|(row, _)| row);
            if cursor_row != Some(d.cursor_row) {
                if d.cursor_row < rows {
                    self.mask[rect.row + d.cursor_row] = true;
                }
                if let Some(row) = cursor_row.filter(|r| *r < rows) {
                    self.mask[rect.row + row] = true;
                }
            }
            // The overlay has to be cleared where it was and drawn where it is.
            let selection = pane.selection();
            if selection != d.selection {
                let mut span = (0usize, 0usize);
                for sel in [selection, d.selection].into_iter().flatten() {
                    if selection_rows(grid, sel, rows, &mut span) {
                        for m in &mut self.mask[rect.row + span.0..=rect.row + span.1] {
                            *m = true;
                        }
                    }
                }
            }
            idx += 1;
        });
        self.pane_damage = prev;
    }

    /// Record what this frame painted (per pane) so the next damage repaint
    /// knows what to compare against. Reuses the snapshot buffers (NFR-113).
    fn commit_damage(&mut self, app: &App) {
        self.damage.active_tab = app.active_index();
        if let Some(tab) = app.active_tab() {
            self.shape = (tab.pane_count(), tab.cols(), tab.rows());
            let mut snapshot = std::mem::take(&mut self.pane_damage);
            snapshot.clear();
            tab.for_each_pane_ref(&mut |pane: &Pane, _| {
                let grid = pane.grid();
                snapshot.push(PaneDamage {
                    grid_cols: grid.cols(),
                    grid_rows: grid.rows(),
                    cursor_row: cursor_cell(pane, grid).map_or(usize::MAX, |(row, _)| row),
                    selection: pane.selection(),
                });
            });
            self.pane_damage = snapshot;
        } else {
            self.shape = (0, 0, 0);
            self.pane_damage.clear();
        }
        self.damage.painted = true;
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

    /// The mouse-selection column span painted on viewport row `y`
    /// (`None` when the row has no selection). Computed once per row in
    /// [`Renderer::paint_frame`] and shared by the overlay fill and the
    /// glyph keys, so a blitted tile's baked highlight and the overlay
    /// cannot disagree.
    fn row_selection(pane: &Pane, y: usize, n_cols: usize) -> Option<(usize, usize)> {
        let abs = pane.grid().abs_of_view(y)?;
        let (lo, hi) = pane.selection()?.cols_on(abs, n_cols)?;
        (hi > lo).then_some((lo, hi))
    }

    /// Fill the mouse-selection span on one row of the pane box that starts
    /// at (`left`, `top`) (drawn between the row background and the row
    /// text, so glyphs stay legible).
    fn paint_row_selection(
        &mut self,
        left: f32,
        top: f32,
        span: Option<(usize, usize)>,
        theme: &Theme,
    ) {
        let Some((lo, hi)) = span else {
            return;
        };
        self.fill_rect(
            left + lo as f32 * self.metrics.cell_width,
            top,
            left + hi as f32 * self.metrics.cell_width,
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

    /// Fill the row's cell backgrounds, run by run, inside the pane box that
    /// starts at (`left`, `top`).
    fn paint_row_backgrounds(&mut self, left: f32, top: f32, cells: &[Cell], theme: &Theme) {
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
            self.fill_rect(
                left + x as f32 * self.metrics.cell_width,
                top,
                left + end as f32 * self.metrics.cell_width,
                top + self.metrics.cell_height,
                bg,
            );
            x = end;
        }
    }

    /// Paint one row's glyphs inside the pane box that starts at
    /// (`left`, `top`). With an atlas, a cell whose tile is already
    /// rendered is a single `DrawBitmap`; glyphs the atlas has not finished
    /// yet fall back to style+colour runs of `DrawText`. The UTF-16 text is
    /// built into the reused `self.utf16` buffer: the hot path allocates
    /// nothing per frame (NFR-113).
    fn paint_row_text(
        &mut self,
        left: f32,
        top: f32,
        cells: &[Cell],
        span: Option<(usize, usize)>,
        theme: &Theme,
    ) {
        let cw = self.metrics.cell_width;
        let ch = self.metrics.cell_height;
        let y_top = top + (ch - self.metrics.text_height) / 2.0;
        let atlas = self.atlas.take();
        let mut units = std::mem::take(&mut self.utf16);
        let mut i = 0;
        while i < cells.len() {
            let cell = &cells[i];
            if cell.char == ' ' || cell.attributes.contains(CellAttribute::HIDDEN) {
                i += 1;
                continue;
            }
            let key = Self::cell_key(cell, i, span, theme);
            let tile = key
                .as_ref()
                .and_then(|key| atlas.as_ref().and_then(|atlas| atlas.lookup(key)));
            if let Some(tile) = tile {
                if let Some(atlas) = atlas.as_ref() {
                    self.blit_glyph(atlas, tile, left + i as f32 * cw, top);
                }
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
                // A neighbour with a ready tile is blitted on its own: do
                // not fold it into the text run.
                let blitted = Self::cell_key(next, end, span, theme).is_some_and(|key| {
                    atlas
                        .as_ref()
                        .is_some_and(|atlas| atlas.lookup(&key).is_some())
                });
                if blitted {
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
                left: left + i as f32 * cw,
                top: y_top,
                right: left + end as f32 * cw,
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
        self.atlas = atlas;
    }

    /// Blit one cached tile into the cell box at `left`/`top`.
    fn blit_glyph(&self, atlas: &GlyphAtlas, tile: Tile, left: f32, top: f32) {
        atlas.blit(
            &self.target,
            tile,
            left,
            top,
            self.metrics.cell_width,
            self.metrics.cell_height,
        );
    }

    /// Atlas key for one cell: its character, text format, and effective
    /// colours, with the mouse-selection highlight baked into the
    /// background. `None` for cells that carry no glyph.
    fn cell_key(
        cell: &Cell,
        x: usize,
        span: Option<(usize, usize)>,
        theme: &Theme,
    ) -> Option<GlyphKey> {
        if cell.char == ' ' || cell.attributes.contains(CellAttribute::HIDDEN) {
            return None;
        }
        let (fg, bg) = Self::effective_colors(cell, theme);
        let bg = match span {
            Some((lo, hi)) if x >= lo && x < hi => theme.selection,
            _ => bg,
        };
        let format = u8::try_from(format_index(cell.attributes)).unwrap_or(0);
        Some(GlyphKey::new(cell.char, format, fg, bg))
    }

    fn paint_row_decorations(&mut self, left0: f32, top: f32, cells: &[Cell], theme: &Theme) {
        let cw = self.metrics.cell_width;
        let ch = self.metrics.cell_height;
        for (x, cell) in cells.iter().enumerate() {
            if cell.char == ' ' || cell.attributes.contains(CellAttribute::HIDDEN) {
                continue;
            }
            let (fg, _) = Self::effective_colors(cell, theme);
            let left = left0 + x as f32 * cw;
            let right = left + cw;
            if cell.attributes.contains(CellAttribute::UNDERLINE) {
                let yline = top + ch - 1.0;
                self.stroke_line(left, yline, right, yline, fg);
            }
            if cell.attributes.contains(CellAttribute::STRIKE) {
                let yline = top + ch / 2.0;
                self.stroke_line(left, yline, right, yline, fg);
            }
        }
    }

    /// Paint one pane's cursor: the solid block on the focused pane, a
    /// hollow outline elsewhere (SPLIT-003 focus cue, SPLIT-015).
    fn paint_cursor(&mut self, pane: &Pane, rect: Rect, focused: bool, theme: &Theme) {
        let grid = pane.grid();
        if grid.scroll_offset() != 0 || !pane.cursor_visible() {
            return;
        }
        let row = pane.parser().cursor_row;
        let col = pane.parser().cursor_col;
        if row >= rect.rows || col >= rect.cols {
            return;
        }
        let top = self.tab_bar_height + (rect.row + row) as f32 * self.metrics.cell_height;
        let left = (rect.col + col) as f32 * self.metrics.cell_width;
        let right = left + self.metrics.cell_width;
        let bottom = top + self.metrics.cell_height;
        if focused {
            self.fill_rect(left, top, right, bottom, theme.cursor);
        } else {
            // Hollow block: outline only, so the focused pane's block stands out.
            self.stroke_line(left, top, right, top, theme.cursor);
            self.stroke_line(left, bottom, right, bottom, theme.cursor);
            self.stroke_line(left, top, left, bottom, theme.cursor);
            self.stroke_line(right, top, right, bottom, theme.cursor);
        }
    }

    /// Fill the separator lines between touching panes (SPLIT-014, SPLIT-015):
    /// 1 DIP wide, `split_border` everywhere, `split_focus` on the focused
    /// pane's edges. Unfocused edges paint first so the focused color wins a
    /// shared boundary. Allocation-free.
    fn paint_split_borders(&mut self, tab: &Tab, theme: &Theme) {
        let cw = self.metrics.cell_width;
        let ch = self.metrics.cell_height;
        let focused = tab.focused_index().unwrap_or(usize::MAX);
        for pass_focus in [false, true] {
            let mut idx = 0usize;
            tab.for_each_pane_ref(&mut |_: &Pane, rect: Rect| {
                if (idx == focused) == pass_focus {
                    let x = rect.col as f32 * cw;
                    let y = self.tab_bar_height + rect.row as f32 * ch;
                    let w = rect.cols as f32 * cw;
                    let pane_height = rect.rows as f32 * ch;
                    let color = if pass_focus {
                        theme.split_focus
                    } else {
                        theme.split_border
                    };
                    if rect.col > 0 && leaf_at(tab, rect.col - 1, rect.row).is_some() {
                        self.fill_rect(x, y, x + 1.0, y + pane_height, color);
                    }
                    if rect.col + rect.cols < tab.cols()
                        && leaf_at(tab, rect.col + rect.cols, rect.row).is_some()
                    {
                        self.fill_rect(x + w - 1.0, y, x + w, y + pane_height, color);
                    }
                    if rect.row > 0 && leaf_at(tab, rect.col, rect.row - 1).is_some() {
                        self.fill_rect(x, y, x + w, y + 1.0, color);
                    }
                    if rect.row + rect.rows < tab.rows()
                        && leaf_at(tab, rect.col, rect.row + rect.rows).is_some()
                    {
                        self.fill_rect(x, y + pane_height - 1.0, x + w, y + pane_height, color);
                    }
                }
                idx += 1;
            });
        }
    }

    /// Paint a title chip in each pane's top-left corner (SPLIT-011): the
    /// pane's OSC-set title, on panes that have one. Reuses the tab-bar
    /// label buffers, so the hot path stays allocation-free (NFR-113).
    fn paint_pane_titles(&mut self, tab: &Tab, theme: &Theme) {
        let cw = self.metrics.cell_width;
        let ch = self.metrics.cell_height;
        let focused = tab.focused_index().unwrap_or(usize::MAX);
        let mut units = std::mem::take(&mut self.utf16);
        let mut label = std::mem::take(&mut self.label);
        let mut idx = 0usize;
        tab.for_each_pane_ref(&mut |pane: &Pane, rect: Rect| {
            let is_focus = idx == focused;
            let Some(title) = pane.title() else {
                idx += 1;
                return;
            };
            let x = rect.col as f32 * cw;
            let y = self.tab_bar_height + rect.row as f32 * ch;
            let w = rect.cols as f32 * cw;
            label.clear();
            tabbar::fit_title_into(title, (w - 2.0).max(0.0) - TAB_CHROME, cw, &mut label);
            if label.is_empty() {
                idx += 1;
                return;
            }
            let chip_w = label.chars().count() as f32 * cw + TAB_CHROME;
            let bg = if is_focus {
                mix(theme.tab_bar, theme.split_focus, 0.25)
            } else {
                theme.tab_bar
            };
            self.fill_rect(
                x + 1.0,
                y + 1.0,
                x + 1.0 + chip_w,
                y + 1.0 + self.metrics.text_height,
                bg,
            );
            units.clear();
            units.extend(label.encode_utf16());
            let rect_box = D2D_RECT_F {
                left: x + 1.0 + TAB_CHROME / 2.0,
                top: y + 1.0,
                right: x + 1.0 + chip_w - TAB_CHROME / 2.0,
                bottom: y + 1.0 + self.metrics.text_height,
            };
            let color = if is_focus {
                theme.foreground
            } else {
                mix(theme.foreground, theme.tab_bar, 0.45)
            };
            if let Some(brush) = self.brush(color) {
                let format = &self.formats[0];
                unsafe {
                    self.target.DrawText(
                        &units,
                        format,
                        &rect_box,
                        &brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }
            idx += 1;
        });
        units.clear();
        self.utf16 = units;
        self.label = label;
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
