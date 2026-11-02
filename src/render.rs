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
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_FEATURE, DWRITE_FONT_FEATURE_TAG,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE, DWriteCreateFactory, IDWriteFactory,
    IDWriteFontCollection, IDWriteTextFormat, IDWriteTextLayout, IDWriteTypography,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::PCWSTR;
use windows_numerics::Vector2;

use crate::app::App;
use crate::color::Color;
use crate::config::{CursorShape, FontConfig, TabBarPlacement};
use crate::glyph_cache::{self, GlyphAtlas, GlyphKey, Tile};
use crate::grid::{Cell, CellAttribute, Grid};
use crate::pane::Pane;
use crate::search::SearchState;
use crate::selection::Selection;
use crate::split::Rect;
use crate::tab::Tab;
use crate::tabbar::{self, TAB_CHROME};
use crate::theme::Theme;

/// Search overlay height at the bottom of the client area (SEARCH-001..015).
/// It is painted over the grid, so opening search does not resize the grid
/// or the ConPTY.
pub const SEARCH_BAR_HEIGHT: f32 = 24.0;

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
fn font_feature_tag(tag: &str) -> Option<DWRITE_FONT_FEATURE_TAG> {
    let bytes = tag.as_bytes();
    if bytes.len() != 4 {
        return None;
    }
    Some(DWRITE_FONT_FEATURE_TAG(u32::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
    ])))
}

/// Parse a font feature string into `(tag, enabled)` pairs (FONT-011).
///
/// Accepted forms: `tag value`, `tag=value`, and comma-separated lists, so
/// `calt 0`, `calt=0`, `calt=0,liga=0` and a bare `calt,liga` (a list
/// enables each tag) all work.
fn parse_font_features(text: &str) -> Vec<(String, bool)> {
    let normalized = text.replace(['=', ','], " ");
    let mut tokens = normalized.split_whitespace().peekable();
    let mut out = Vec::new();
    while let Some(tag) = tokens.next() {
        let value = tokens.peek().map(|word| word.to_ascii_lowercase());
        let enabled = match value.as_deref() {
            // A value word belongs to this tag; any other token starts the
            // next feature, which is why a bare list enables everything.
            Some(word) if is_feature_value(word) => {
                tokens.next();
                !matches!(word, "0" | "off" | "false" | "no")
            }
            _ => true,
        };
        out.push((tag.to_ascii_lowercase(), enabled));
    }
    out
}

fn is_feature_value(word: &str) -> bool {
    matches!(
        word,
        "0" | "1" | "on" | "off" | "true" | "false" | "yes" | "no"
    )
}

fn build_typography(
    dwrite: &IDWriteFactory,
    font: &FontConfig,
) -> anyhow::Result<Option<IDWriteTypography>> {
    let mut features: Vec<(DWRITE_FONT_FEATURE_TAG, u32)> = Vec::new();
    for (tag, enabled) in parse_font_features(&font.features) {
        let Some(tag) = font_feature_tag(&tag) else {
            continue;
        };
        features.push((tag, u32::from(enabled)));
    }
    if features.is_empty() {
        return Ok(None);
    }
    let typography: IDWriteTypography = unsafe { dwrite.CreateTypography() }?;
    for (tag, parameter) in features {
        let feature = DWRITE_FONT_FEATURE {
            nameTag: tag,
            parameter,
        };
        unsafe { typography.AddFontFeature(feature) }?;
    }
    Ok(Some(typography))
}

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
    dwrite: IDWriteFactory,
    typography: Option<IDWriteTypography>,
    formats: [IDWriteTextFormat; 4],
    metrics: CellMetrics,
    dpi: f32,
    /// Tab-bar strip height in DIPs; 0.0 when the bar is hidden
    /// (`tab-bar = "auto"` with one tab, or `"hidden"`, TAB-010).
    tab_bar_height: f32,
    tab_bar_placement: TabBarPlacement,
    tab_show_profiles: bool,
    tab_close_button: bool,
    cursor_shape: CursorShape,
    cursor_blink: bool,
    cursor_blink_visible: bool,
    split_border: bool,
    split_padding: f32,
    /// BELL-003/004: while a bell is unacknowledged, the tabs that rang and
    /// the focused pane are drawn in the highlight colour.
    bell_flash: bool,
    bell_tabs: Vec<bool>,
    bell_focused_pane: bool,
    /// `[window.padding]` in DIPs: the grid is inset by this much, the tab
    /// bar strip is not (WINDOW-011).
    pad_x: f32,
    pad_y: f32,
    brushes: HashMap<u32, ID2D1SolidColorBrush>,
    /// Reused UTF-16 buffer: the text paint path fills this instead of
    /// building a `String` + `Vec<u16>` per run (NFR-113: no allocations in
    /// the render hot path).
    utf16: Vec<u16>,
    /// Reused tab-bar buffers: titles, layout, and the fitted label.
    titles: Vec<String>,
    layout: tabbar::Layout,
    label: String,
    /// Reused fitted label for the search bar (SEARCH-004 display).
    search_label: String,
    /// Reused rename-prompt label (TAB-013).
    prompt_label: String,
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
    /// Ligatures on (FONT-010): the atlas is per-cell, so ligature text is
    /// shaped as runs through DirectWrite instead, with no atlas at all.
    ligatures: bool,
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
        let typography = build_typography(&dwrite, font)?;
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
            dwrite,
            typography,
            formats,
            metrics,
            dpi,
            tab_bar_height,
            tab_bar_placement: TabBarPlacement::Top,
            tab_show_profiles: true,
            tab_close_button: true,
            cursor_shape: CursorShape::Block,
            cursor_blink: true,
            cursor_blink_visible: true,
            split_border: true,
            split_padding: 1.0,
            bell_flash: false,
            bell_tabs: Vec::new(),
            bell_focused_pane: false,
            pad_x: 0.0,
            pad_y: 0.0,
            brushes: HashMap::new(),
            utf16: Vec::with_capacity(cols.max(1) * 2),
            titles: Vec::new(),
            layout: tabbar::Layout::default(),
            label: String::new(),
            search_label: String::new(),
            prompt_label: String::new(),
            mask: Vec::new(),
            damage: Damage::default(),
            pane_damage: Vec::new(),
            shape: (0, 0, 0),
            atlas: None,
            atlas_failed: false,
            ligatures: font.ligatures,
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

    /// Configure tab-bar strip options.
    pub fn set_tab_bar_options(
        &mut self,
        placement: TabBarPlacement,
        show_profiles: bool,
        close_button: bool,
    ) {
        self.tab_bar_placement = placement;
        self.tab_show_profiles = show_profiles;
        self.tab_close_button = close_button;
    }

    /// Configure cursor drawing.
    pub fn set_cursor_options(&mut self, shape: CursorShape, blink: bool) {
        self.cursor_shape = shape;
        self.cursor_blink = blink;
        self.cursor_blink_visible = true;
    }

    /// Toggle cursor blink state.
    pub fn set_cursor_blink_visible(&mut self, visible: bool) {
        self.cursor_blink_visible = visible;
    }

    /// Swap in a new font (SHELL-019 per-profile font): text formats, cell
    /// metrics, font features and the glyph atlas are rebuilt. The grid the
    /// window fits changes with the metrics, so the caller resizes every tab
    /// to [`Renderer::grid_size`] afterwards.
    pub fn set_font(&mut self, font: &FontConfig) -> anyhow::Result<()> {
        let (_family, formats, metrics) = build_font_resources(&self.dwrite, self.dpi, font)?;
        self.typography = build_typography(&self.dwrite, font)?;
        self.formats = formats;
        self.metrics = metrics;
        self.ligatures = font.ligatures;
        self.atlas = None;
        Ok(())
    }

    /// Configure split chrome (SPLIT-014, CONFIG-013).
    pub fn set_split_options(&mut self, border: bool, padding: u32) {
        self.split_border = border;
        self.split_padding = padding as f32;
    }

    /// BELL-003/004: which tabs to flash, and whether the focused pane of the
    /// active tab flashes too.
    pub fn set_bell_state(&mut self, flash: bool, tabs: &[bool], focused_pane: bool) {
        self.bell_flash = flash;
        self.bell_tabs.clear();
        self.bell_tabs.extend_from_slice(tabs);
        self.bell_focused_pane = focused_pane;
    }

    /// `[window.padding]` (WINDOW-011). Callers re-read [`Renderer::grid_size`]
    /// afterwards: the inset takes grid cells away.
    /// shortcut: the inset arithmetic has no test (it needs a live render
    /// target); add one when the renderer gets a software-target test.
    pub fn set_padding(&mut self, x: u32, y: u32) {
        self.pad_x = x as f32;
        self.pad_y = y as f32;
    }

    /// Left edge of the grid in client DIPs (the window padding).
    #[must_use]
    pub fn grid_left(&self) -> f32 {
        self.pad_x
    }

    /// Top padding above the grid, on top of the tab-bar strip (WINDOW-011).
    #[must_use]
    pub fn grid_pad_y(&self) -> f32 {
        self.pad_y
    }

    fn grid_top(&self) -> f32 {
        let bar = if self.tab_bar_placement == TabBarPlacement::Top {
            self.tab_bar_height
        } else {
            0.0
        };
        bar + self.pad_y
    }

    fn draw_units(
        &self,
        units: &[u16],
        format: &IDWriteTextFormat,
        rect: D2D_RECT_F,
        brush: &ID2D1SolidColorBrush,
    ) {
        if units.is_empty() {
            return;
        }
        if let Some(typography) = self.typography.as_ref() {
            // One cell of slack in the layout width: shaping a run at the
            // exact run width can wrap its last glyph to a second line, and
            // the clip is one line tall.
            // shortcut: the layout is built per run per frame; cache layouts
            // by text+format if the shaped path ever misses the frame budget.
            let width = (rect.right - rect.left).max(1.0) + self.metrics.cell_width;
            let height = (rect.bottom - rect.top).max(1.0);
            if let Ok(layout) =
                unsafe { self.dwrite.CreateTextLayout(units, format, width, height) }
            {
                let range = DWRITE_TEXT_RANGE {
                    startPosition: 0,
                    length: units.len() as u32,
                };
                unsafe {
                    let _ = layout.SetTypography(typography, range);
                    self.target.DrawTextLayout(
                        Vector2 {
                            X: rect.left,
                            Y: rect.top,
                        },
                        &layout,
                        brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                }
                return;
            }
        }
        unsafe {
            self.target.DrawText(
                units,
                format,
                &rect,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            )
        };
    }

    fn tab_bar_top(&self, height: f32) -> f32 {
        if self.tab_bar_placement == TabBarPlacement::Top {
            0.0
        } else {
            (height - self.tab_bar_height).max(0.0)
        }
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
        let w = (px_to_dips(px.width, self.dpi) - 2.0 * self.pad_x).max(0.0);
        let h = (px_to_dips(px.height, self.dpi) - self.tab_bar_height - 2.0 * self.pad_y).max(0.0);
        (
            (w / self.metrics.cell_width).floor().max(1.0) as usize,
            (h / self.metrics.cell_height).floor().max(1.0) as usize,
        )
    }

    /// Paint the whole client area: tab bar strip, active pane viewport,
    /// cursor, and the search overlay when `search` is active. This is the
    /// full repaint, used for system-initiated `WM_PAINT` (expose, resize,
    /// coming back from occluded). Never panics on Direct2D failure (NFR-216).
    pub fn paint(
        &mut self,
        app: &App,
        theme: &Theme,
        search: Option<&SearchState>,
        prompt: Option<&str>,
    ) {
        self.paint_frame(app, theme, search, prompt, true);
    }

    /// Paint only what changed since the last frame (NFR-113 damage-only
    /// redraw): the rows the grid wrote, plus the rows a cursor move or a
    /// selection change touched, plus the tab-bar strip and search overlay.
    /// Falls back to a full repaint when the tab, the grid size, or the whole
    /// grid changed.
    ///
    /// The caller clears the grid damage flags afterwards
    /// (see [`crate::app::App::clear_grid_damage`]); a dropped frame keeps
    /// them set, so the next call repaints everything.
    pub fn paint_damage(
        &mut self,
        app: &App,
        theme: &Theme,
        search: Option<&SearchState>,
        prompt: Option<&str>,
    ) {
        self.paint_frame(app, theme, search, prompt, false);
    }

    /// The tab-bar geometry the last frame painted, for hit testing.
    #[must_use]
    pub fn tab_layout(&self) -> tabbar::Layout {
        self.layout.clone()
    }

    /// Caret tip (top-left of the focused pane's cursor cell) in client DIPs,
    /// used to place the IME composition window at the caret (WIN-006).
    /// `None` when the focused pane's cursor is scrolled away.
    #[must_use]
    pub fn cursor_dip_position(&self, app: &App) -> Option<(f32, f32)> {
        let tab = app.active_tab()?;
        let focused = tab.focused_index()?;
        let mut found = None;
        let mut idx = 0usize;
        tab.for_each_pane_ref(&mut |pane: &Pane, rect: Rect| {
            if idx == focused {
                let parser = pane.parser();
                found = Some((
                    parser.cursor_row,
                    parser.cursor_col,
                    pane.grid().scroll_offset(),
                    rect,
                ));
            }
            idx += 1;
        });
        let (row, col, scroll, rect) = found?;
        if scroll != 0 || row >= rect.rows || col >= rect.cols {
            return None;
        }
        let pad = self.split_padding;
        let x = self.grid_left() + (rect.col + col) as f32 * self.metrics.cell_width + pad;
        let y = self.grid_top() + (rect.row + row) as f32 * self.metrics.cell_height + pad;
        Some((x, y))
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
        ((width - 2.0 * self.pad_x).max(0.0) / self.metrics.cell_width)
            .floor()
            .max(1.0) as usize
    }

    fn paint_frame(
        &mut self,
        app: &App,
        theme: &Theme,
        search: Option<&SearchState>,
        prompt: Option<&str>,
        force_full: bool,
    ) {
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
        // FONT-010: an atlas tile is one cell, so a ligature can never form
        // in it. With ligatures on, every run goes through the shaped
        // `draw_units` path and no atlas is built.
        // shortcut: all-or-nothing; upgrade to bypassing the atlas only for
        // runs whose text can form a ligature if the shaped path ever misses
        // the frame budget.
        let mut atlas = (!self.ligatures && self.ensure_atlas())
            .then(|| self.atlas.take())
            .flatten();
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
            let mut pane_idx = 0usize;
            tab.for_each_pane_ref(&mut |pane: &Pane, rect: Rect| {
                let idx = pane_idx;
                pane_idx += 1;
                // `[split.padding]` is a visual gap: pane content starts
                // `pad` DIPs in, so neighbours are separated by it.
                // shortcut: content is offset, not reflowed; upgrade if a
                // pane ever needs its own pixel size.
                let pad = self.split_padding;
                let left = self.grid_left() + rect.col as f32 * cw + pad;
                let top0 = self.grid_top() + rect.row as f32 * ch + pad;
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
                        left + rect.cols as f32 * cw - 2.0 * pad,
                        top + ch,
                        theme.background,
                    );
                    let span = Self::row_selection(pane, y, n);
                    self.paint_row_backgrounds(left, top, cells, theme);
                    self.paint_row_selection(left, top, span, theme);
                    if let Some(search) = search {
                        self.paint_row_search(left, top, grid, y, idx, search, theme);
                    }
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
            self.paint_tab_bar(app, theme, width, height);
        }
        // TAB-013: the rename prompt takes the overlay strip from search.
        if let Some(prompt) = prompt {
            self.prompt_label = prompt.to_string();
            self.paint_prompt_bar(theme, width, height);
        } else if let Some(search) = search {
            self.paint_search_bar(search, theme, width, height);
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

    /// Paint every visible search match on row `view_row` of pane `pane_idx`
    /// (SEARCH-004). Matches in scrollback are not painted until they are
    /// scrolled into view. Drawn before the row text so glyphs stay legible.
    #[allow(
        clippy::too_many_arguments,
        reason = "SEARCH-004 row geometry and search state are all needed"
    )]
    fn paint_row_search(
        &mut self,
        left: f32,
        top: f32,
        grid: &Grid,
        view_row: usize,
        pane_idx: usize,
        search: &SearchState,
        theme: &Theme,
    ) {
        let cw = self.metrics.cell_width;
        let ch = self.metrics.cell_height;
        for match_ in &search.matches {
            if match_.pane != pane_idx || grid.view_of_abs(match_.abs) != Some(view_row) {
                continue;
            }
            self.fill_rect(
                left + match_.lo as f32 * cw,
                top,
                left + match_.hi as f32 * cw,
                top + ch,
                theme.search_highlight,
            );
        }
    }

    /// Paint the search overlay at the bottom of the client area
    /// (SEARCH-001..015): query, current match, match count, option states,
    /// and a regex error. It overlays the grid and does not resize it.
    /// The inline tab-rename prompt (TAB-013), on the search strip.
    fn paint_prompt_bar(&mut self, theme: &Theme, width: f32, height: f32) {
        let bar_h = SEARCH_BAR_HEIGHT.min(height.max(0.0));
        if bar_h <= 0.0 {
            return;
        }
        let label = std::mem::take(&mut self.prompt_label);
        let bottom = height - self.tab_bar_height.min(height.max(0.0));
        let top = (bottom - bar_h).max(0.0);
        self.fill_rect(0.0, top, width, bottom, theme.tab_bar);
        tabbar::fit_title_into(
            &label,
            (width - 16.0).max(0.0),
            self.metrics.cell_width,
            &mut self.search_label,
        );
        if !self.search_label.is_empty() {
            let mut units = std::mem::take(&mut self.utf16);
            units.clear();
            units.extend(self.search_label.encode_utf16());
            let text_top = top + (bar_h - self.metrics.text_height) / 2.0;
            let rect = D2D_RECT_F {
                left: 8.0,
                top: text_top,
                right: width - 8.0,
                bottom: text_top + self.metrics.text_height,
            };
            if let Some(brush) = self.brush(theme.foreground) {
                let format = &self.formats[0];
                self.draw_units(&units, format, rect, &brush);
            }
            self.utf16 = units;
        }
        self.prompt_label = label;
    }

    fn paint_search_bar(&mut self, search: &SearchState, theme: &Theme, width: f32, height: f32) {
        let bar_h = SEARCH_BAR_HEIGHT.min(height.max(0.0));
        if bar_h <= 0.0 {
            return;
        }
        let tab_bottom = self.tab_bar_height.min(height.max(0.0));
        let bottom = height - tab_bottom;
        let top = (bottom - bar_h).max(0.0);
        self.fill_rect(0.0, top, width, bottom, theme.tab_bar);
        let mut raw = std::mem::take(&mut self.label);
        raw.clear();
        raw.push_str("Search: ");
        raw.push_str(&search.query);
        raw.push_str("  ");
        if search.matches.is_empty() {
            raw.push_str("0/0");
        } else {
            raw.push_str(&(search.current + 1).to_string());
            raw.push('/');
            raw.push_str(&search.matches.len().to_string());
        }
        raw.push_str("  case=");
        raw.push_str(if search.options.case_sensitive {
            "on"
        } else {
            "off"
        });
        raw.push_str(" word=");
        raw.push_str(if search.options.whole_word {
            "on"
        } else {
            "off"
        });
        raw.push_str(" regex=");
        raw.push_str(if search.options.regex { "on" } else { "off" });
        if let Some(err) = &search.error {
            raw.push_str("  ");
            raw.push_str(err);
        }
        tabbar::fit_title_into(
            &raw,
            (width - 16.0).max(0.0),
            self.metrics.cell_width,
            &mut self.search_label,
        );
        if !self.search_label.is_empty() {
            let mut units = std::mem::take(&mut self.utf16);
            units.clear();
            units.extend(self.search_label.encode_utf16());
            let text_top = top + (bar_h - self.metrics.text_height) / 2.0;
            let rect = D2D_RECT_F {
                left: 8.0,
                top: text_top,
                right: width - 8.0,
                bottom: text_top + self.metrics.text_height,
            };
            if let Some(brush) = self.brush(theme.foreground) {
                let format = &self.formats[0];
                self.draw_units(&units, format, rect, &brush);
            }
            self.utf16 = units;
        }
        self.label = raw;
    }

    /// Paint the tab bar strip: bar background, per-tab titles (active
    /// tab highlighted), and the `+` new-tab button (tab bar P0).
    /// Every buffer it needs is reused from the renderer, so a frame with
    /// the bar visible allocates nothing (NFR-113).
    fn paint_tab_bar(&mut self, app: &App, theme: &Theme, width: f32, height: f32) {
        let bar_top = self.tab_bar_top(height);
        let bar_bottom = bar_top + self.tab_bar_height;
        self.fill_rect(0.0, bar_top, width, bar_bottom, theme.tab_bar);
        // Titles: fill the reused slots with the reused scratch String,
        // swapping buffers so each slot keeps its own capacity.
        let cw = self.metrics.cell_width;
        let mut label = std::mem::take(&mut self.label);
        let mut units = std::mem::take(&mut self.utf16);
        let titles = &mut self.titles;
        titles.resize(app.tabs().len(), String::new());
        for (slot, tab) in titles.iter_mut().zip(app.tabs()) {
            label.clear();
            if self.tab_show_profiles && !tab.profile().is_empty() {
                label.push_str(tab.profile());
                label.push_str(" Â· ");
            }
            label.push_str(tab.title());
            if self.tab_close_button {
                label.push_str("  Ã");
            }
            std::mem::swap(slot, &mut label);
        }
        tabbar::layout_into(
            &self.titles,
            width,
            self.metrics.cell_width,
            &mut self.layout,
        );
        let active = app.active_index();
        let y_top = bar_top + (self.tab_bar_height - self.metrics.text_height) / 2.0;
        let plus = self.layout.plus;
        let tabs = std::mem::take(&mut self.layout.tabs);
        for (i, &(lo, hi)) in tabs.iter().enumerate() {
            if hi <= lo {
                continue;
            }
            let tab = &app.tabs()[i];
            if let Some(color) = tab.color() {
                let bg = if i == active {
                    mix(color, theme.tab_active, 0.25)
                } else {
                    color
                };
                self.fill_rect(lo, bar_top, hi, bar_bottom, bg);
            }
            if i == active {
                self.fill_rect(lo, bar_top, hi, bar_bottom, theme.tab_active);
            }
            if self.bell_flash && self.bell_tabs.get(i) == Some(&true) {
                self.fill_rect(
                    lo,
                    bar_top,
                    hi,
                    bar_bottom,
                    mix(theme.search_highlight, theme.tab_bar, 0.25),
                );
            }
            let close_w = if self.tab_close_button { cw * 1.5 } else { 0.0 };
            tabbar::fit_title_into(
                &self.titles[i],
                (hi - lo) - TAB_CHROME - close_w,
                cw,
                &mut label,
            );
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
            self.draw_units(&units, format, rect, &brush);
            if self.tab_close_button {
                let close_w = cw * 1.5;
                units.clear();
                units.extend("\u{d7}".encode_utf16());
                let close_rect = D2D_RECT_F {
                    left: hi - close_w,
                    top: y_top,
                    right: hi,
                    bottom: y_top + self.metrics.text_height,
                };
                self.draw_units(&units, format, close_rect, &brush);
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
                self.draw_units(&units, format, rect, &brush);
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
            self.draw_units(&units, format, rect, &brush);
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
        if focused && self.cursor_blink && !self.cursor_blink_visible {
            return;
        }
        let pad = self.split_padding;
        let top = self.grid_top() + (rect.row + row) as f32 * self.metrics.cell_height + pad;
        let left = self.grid_left() + (rect.col + col) as f32 * self.metrics.cell_width + pad;
        let right = left + self.metrics.cell_width;
        let bottom = top + self.metrics.cell_height;
        if !focused {
            self.stroke_line(left, top, right, top, theme.cursor);
            self.stroke_line(left, bottom, right, bottom, theme.cursor);
            self.stroke_line(left, top, left, bottom, theme.cursor);
            self.stroke_line(right, top, right, bottom, theme.cursor);
            return;
        }
        match self.cursor_shape {
            CursorShape::Block => self.fill_rect(left, top, right, bottom, theme.cursor),
            CursorShape::Bar => self.fill_rect(left, top, left + 2.0, bottom, theme.cursor),
            CursorShape::Underline => {
                self.fill_rect(left, bottom - 2.0, right, bottom, theme.cursor)
            }
        }
    }

    /// Fill the separator lines between touching panes (SPLIT-014, SPLIT-015):
    /// 1 DIP wide, `split_border` everywhere, `split_focus` on the focused
    /// pane's edges. Unfocused edges paint first so the focused color wins a
    /// shared boundary. Allocation-free.
    fn paint_split_borders(&mut self, tab: &Tab, theme: &Theme) {
        if !self.split_border {
            return;
        }
        let cw = self.metrics.cell_width;
        let ch = self.metrics.cell_height;
        let focused = tab.focused_index().unwrap_or(usize::MAX);
        for pass_focus in [false, true] {
            let mut idx = 0usize;
            tab.for_each_pane_ref(&mut |_: &Pane, rect: Rect| {
                if (idx == focused) == pass_focus {
                    let pad = self.split_padding;
                    let x = self.grid_left() + rect.col as f32 * cw + pad;
                    let y = self.grid_top() + rect.row as f32 * ch + pad;
                    let w = rect.cols as f32 * cw - 2.0 * pad;
                    let pane_height = rect.rows as f32 * ch - pad;
                    let color = if !pass_focus {
                        theme.split_border
                    } else if self.bell_flash && self.bell_focused_pane {
                        theme.search_highlight
                    } else {
                        theme.split_focus
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
            let pad = self.split_padding;
            let x = self.grid_left() + rect.col as f32 * cw + pad;
            let y = self.grid_top() + rect.row as f32 * ch + pad;
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

#[cfg(test)]
mod tests {
    use super::parse_font_features;

    #[test]
    fn font_feature_strings_parse() {
        let cases = [
            ("", vec![]),
            ("calt 0", vec![("calt", false)]),
            ("calt=0", vec![("calt", false)]),
            ("calt=0,liga=0", vec![("calt", false), ("liga", false)]),
            // A bare comma list enables every tag it names.
            ("calt,liga", vec![("calt", true), ("liga", true)]),
            ("calt liga", vec![("calt", true), ("liga", true)]),
            ("CALT OFF", vec![("calt", false)]),
            (
                "nonsense 1 calt on",
                vec![("nonsense", true), ("calt", true)],
            ),
        ];
        for (text, want) in cases {
            let got = parse_font_features(text);
            let pairs: Vec<(&str, bool)> = got.iter().map(|(t, e)| (t.as_str(), *e)).collect();
            assert_eq!(pairs, want, "feature string {text:?}");
        }
    }
}
