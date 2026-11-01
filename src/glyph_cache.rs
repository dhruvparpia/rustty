//! Glyph atlas: renders every unique cell glyph once into an offscreen
//! Direct2D bitmap and blits the finished tiles, so a frame stops
//! re-typesetting text it has already drawn (spec §20 "custom glyph cache /
//! custom font atlas", NFR-113/NFR-114, file map §21).
//!
//! A tile is one terminal cell, fully opaque, keyed by everything that
//! changes its pixels: the character, the text format
//! (plain/bold/italic/bold-italic), the foreground and the background. The
//! mouse-selection highlight is just a background colour, so a highlighted
//! glyph is a tile with the selection colour baked in — no extra blending
//! state, and the highlight looks exactly like the `DrawText` path did.
//!
//! The atlas is a `CreateCompatibleRenderTarget` bitmap of the window's
//! render target, so it inherits the target's DPI: a tile's image coordinates
//! are the target's DIPs and a blit is 1:1 at 100% scaling. Tiles are packed
//! in shelves; when the atlas fills up, the map is dropped and the shelves
//! restart, which is a cheap and correct fallback for a long-lived session.
//!
//! This module is an FFI boundary (NFR-401): every Direct2D call is `unsafe`.
//! It never panics on Direct2D failure (NFR-216): a tile that cannot be
//! rendered is not cached, and the caller draws that glyph with `DrawText`
//! instead ([`GlyphAtlas::request`] reports which tiles are ready).
//!
//! Zero allocation in the hot path (NFR-113): the map, the pending queue and
//! the brush cache are reserved to the atlas capacity at construction, so
//! `request` and `render_pending` never grow a container.

use std::collections::{HashMap, HashSet};

use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE, ID2D1Bitmap,
    ID2D1BitmapRenderTarget, ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{DWRITE_MEASURING_MODE_NATURAL, IDWriteTextFormat};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use crate::color::Color;
use crate::render::{CellMetrics, color_f, color_key, dips_to_px, px_to_dips};

/// Side of the square atlas bitmap, in physical pixels (4 MB at 8 bpp).
const ATLAS_PX: u32 = 1024;

/// Everything that decides the pixels of one terminal cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    /// Character code point.
    pub ch: u32,
    /// Text format index: 0 plain, 1 bold, 2 italic, 3 bold-italic.
    pub format: u8,
    /// Foreground as `0xRRGGBB` (see [`crate::render::color_key`]).
    pub fg: u32,
    /// Background as `0xRRGGBB`; the selection colour is baked in here.
    pub bg: u32,
}

impl GlyphKey {
    /// Key of `ch` drawn with text format `format` in `fg` on `bg`.
    #[must_use]
    pub const fn new(ch: char, format: u8, fg: Color, bg: Color) -> Self {
        Self {
            ch: ch as u32,
            format,
            fg: color_key(fg),
            bg: color_key(bg),
        }
    }
}

/// One rendered glyph: its rectangle inside the atlas bitmap, in DIPs.
#[derive(Debug, Clone, Copy)]
pub struct Tile {
    /// Tile rectangle in atlas coordinates.
    pub rect: D2D_RECT_F,
}

/// A tile that is queued for rendering: key plus the slot it was given.
#[derive(Debug, Clone, Copy)]
struct Pending {
    key: GlyphKey,
    tile: Tile,
}

/// Shelf packer for fixed-size tiles: fills the atlas row by row.
#[derive(Debug)]
struct Shelves {
    tile_w: u32,
    tile_h: u32,
    per_shelf: u32,
    shelves: u32,
    x: u32,
    y: u32,
}

impl Shelves {
    fn new(atlas_px: u32, tile_w: u32, tile_h: u32) -> Self {
        let tile_w = tile_w.max(1);
        let tile_h = tile_h.max(1);
        Self {
            tile_w,
            tile_h,
            per_shelf: (atlas_px / tile_w).max(1),
            shelves: (atlas_px / tile_h).max(1),
            x: 0,
            y: 0,
        }
    }

    /// Number of tiles that fit the atlas.
    fn capacity(&self) -> usize {
        (self.per_shelf as usize).saturating_mul(self.shelves as usize)
    }

    /// Top-left pixel of the next free slot, `None` when the atlas is full.
    fn next_slot(&mut self) -> Option<(u32, u32)> {
        if self.x >= self.per_shelf {
            self.x = 0;
            self.y += 1;
        }
        if self.y >= self.shelves {
            return None;
        }
        let slot = (self.x * self.tile_w, self.y * self.tile_h);
        self.x += 1;
        Some(slot)
    }

    fn restart(&mut self) {
        self.x = 0;
        self.y = 0;
    }
}

/// Atlas counters for the benchmark report ([`crate::perf_bench`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AtlasStats {
    /// Cached tiles.
    pub tiles: usize,
    /// Tiles queued for the next [`GlyphAtlas::render_pending`].
    pub queued: usize,
    /// Tiles rendered since creation.
    pub inserts: u64,
    /// Atlas overflows (whole map drops) since creation.
    pub evictions: u64,
}

/// Offscreen glyph atlas bound to one window render target.
///
/// Drop this together with the brushes when the target's `EndDraw` fails:
/// Direct2D invalidates the target's device-dependent resources
/// ([`crate::render::Renderer`]).
pub struct GlyphAtlas {
    rt: ID2D1BitmapRenderTarget,
    bitmap: ID2D1Bitmap,
    dpi: f32,
    shelves: Shelves,
    map: HashMap<GlyphKey, Tile>,
    pending: Vec<Pending>,
    queued: HashSet<GlyphKey>,
    brushes: HashMap<u32, ID2D1SolidColorBrush>,
    /// Reused UTF-16 buffer for the tile text (2 units: one scalar value).
    units: Vec<u16>,
    inserts: u64,
    evictions: u64,
}

impl GlyphAtlas {
    /// Create the atlas for `parent` (the window's render target) with tiles
    /// the size of one `metrics` cell at `dpi`.
    ///
    /// # Errors
    ///
    /// Fails when the compatible render target or its bitmap cannot be
    /// created; the caller then renders without an atlas.
    pub fn new(
        parent: &ID2D1RenderTarget,
        metrics: &CellMetrics,
        dpi: f32,
    ) -> anyhow::Result<Self> {
        let tile_w = dips_to_px(metrics.cell_width, dpi).max(1);
        let tile_h = dips_to_px(metrics.cell_height, dpi).max(1);
        let pixels = D2D_SIZE_U {
            width: ATLAS_PX,
            height: ATLAS_PX,
        };
        let format = D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_IGNORE,
        };
        let rt = unsafe {
            parent.CreateCompatibleRenderTarget(
                None,
                Some(&pixels),
                Some(&format),
                D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE,
            )
        }?;
        let bitmap = unsafe { rt.GetBitmap() }?;
        let shelves = Shelves::new(ATLAS_PX, tile_w, tile_h);
        // Reserve the containers up front so the hot path never grows one
        // (NFR-113), capped at 4096 tiles so a small font does not pre-pay
        // the whole 1024x1024 tile grid. A session that needs more than 4096
        // distinct tiles pays one `HashMap` growth per doubling, which is the
        // only hot-path allocation the atlas can make.
        let capacity = shelves.capacity().clamp(1, 4096);
        Ok(Self {
            rt,
            bitmap,
            dpi,
            shelves,
            map: HashMap::with_capacity(capacity),
            pending: Vec::with_capacity(capacity),
            queued: HashSet::with_capacity(capacity),
            brushes: HashMap::with_capacity(capacity.saturating_mul(2)),
            units: Vec::with_capacity(2),
            inserts: 0,
            evictions: 0,
        })
    }

    /// Tile for `key` when it is already rendered.
    #[must_use]
    pub fn lookup(&self, key: &GlyphKey) -> Option<Tile> {
        self.map.get(key).copied()
    }

    /// Ask for a tile: the tile when it is ready to blit, `None` when it was
    /// queued for [`GlyphAtlas::render_pending`] (the caller draws that glyph
    /// with `DrawText` this frame).
    pub fn request(&mut self, key: &GlyphKey) -> Option<Tile> {
        if let Some(tile) = self.map.get(key) {
            return Some(*tile);
        }
        if self.queued.contains(key) {
            return None;
        }
        let mut slot = self.shelves.next_slot();
        if slot.is_none() {
            self.evict();
            slot = self.shelves.next_slot();
        }
        let (x, y) = slot?;
        let tile = Tile {
            rect: D2D_RECT_F {
                left: px_to_dips(x, self.dpi),
                top: px_to_dips(y, self.dpi),
                right: px_to_dips(x + self.shelves.tile_w, self.dpi),
                bottom: px_to_dips(y + self.shelves.tile_h, self.dpi),
            },
        };
        self.pending.push(Pending { key: *key, tile });
        self.queued.insert(*key);
        self.inserts += 1;
        None
    }

    /// Render every queued tile into the atlas bitmap, one
    /// `BeginDraw`/`EndDraw` pair for the whole batch. Call this before
    /// [`GlyphAtlas::blit`] needs the tiles.
    ///
    /// A tile whose frame fails to flush is dropped from the queue and not
    /// cached, so the next frame requests it again (NFR-216).
    pub fn render_pending(&mut self, formats: &[IDWriteTextFormat; 4], text_height: f32) {
        let mut pending = std::mem::take(&mut self.pending);
        if pending.is_empty() {
            self.pending = pending;
            return;
        }
        // `BeginDraw` on a render target is infallible (it returns `()`): a
        // target that cannot draw fails at `EndDraw`, which drops the batch.
        unsafe { self.rt.BeginDraw() };
        unsafe { self.rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED) };
        for job in &pending {
            let rect = job.tile.rect;
            // Opaque tile: paint the cell background, then the glyph.
            if let Some(brush) = self.brush(Color::from_hex(job.key.bg)) {
                unsafe { self.rt.FillRectangle(&rect, &brush) };
            }
            let Some(ch) = char::from_u32(job.key.ch) else {
                continue;
            };
            let mut buf = [0u16; 2];
            let len = ch.encode_utf16(&mut buf).len();
            self.units.clear();
            self.units.extend_from_slice(&buf[..len]);
            let cell_h = rect.bottom - rect.top;
            let top = rect.top + (((cell_h - text_height) / 2.0).max(0.0));
            let text = D2D_RECT_F {
                left: rect.left,
                top,
                right: rect.right,
                bottom: top + text_height,
            };
            if let Some(brush) = self.brush(Color::from_hex(job.key.fg)) {
                unsafe {
                    self.rt.DrawText(
                        &self.units,
                        &formats[usize::from(job.key.format)],
                        &text,
                        &brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }
        }
        if unsafe { self.rt.EndDraw(None, None) }.is_ok() {
            for job in &pending {
                self.map.insert(job.key, job.tile);
            }
        }
        pending.clear();
        self.queued.clear();
        self.pending = pending;
    }

    /// Blit `tile` onto `target` into the cell box
    /// `left`..`left + cell_w`, `top`..`top + cell_h`. The destination is the
    /// exact cell metrics, never the tile's (rounded-up) pixel box, so tile
    /// edges stay on the cell grid and a tile never bleeds into its
    /// neighbour. At 100% scaling the box and the tile are the same size and
    /// the blit is 1:1.
    pub fn blit(
        &self,
        target: &ID2D1RenderTarget,
        tile: Tile,
        left: f32,
        top: f32,
        cell_w: f32,
        cell_h: f32,
    ) {
        let dest = D2D_RECT_F {
            left,
            top,
            right: left + cell_w,
            bottom: top + cell_h,
        };
        // `ID2D1RenderTarget::DrawBitmap` returns `()` in the `windows`
        // binding: a failed blit shows as a missing glyph this frame, and
        // the tile is requested again next frame (NFR-216).
        unsafe {
            target.DrawBitmap(
                &self.bitmap,
                Some(&dest),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                Some(&tile.rect),
            );
        }
    }

    /// Atlas counters, for the benchmark ([`crate::perf_bench`]).
    #[must_use]
    pub fn stats(&self) -> AtlasStats {
        AtlasStats {
            tiles: self.map.len(),
            queued: self.queued.len(),
            inserts: self.inserts,
            evictions: self.evictions,
        }
    }

    /// Drop every tile: the atlas is full, so rendering restarts from an
    /// empty shelf. Tiles requested this frame go back into the queue.
    fn evict(&mut self) {
        self.map.clear();
        self.pending.clear();
        self.queued.clear();
        self.shelves.restart();
        self.evictions += 1;
    }

    fn brush(&mut self, color: Color) -> Option<ID2D1SolidColorBrush> {
        let key = color_key(color);
        if let Some(brush) = self.brushes.get(&key) {
            return Some(brush.clone());
        }
        match unsafe { self.rt.CreateSolidColorBrush(&color_f(color), None) } {
            Ok(brush) => {
                self.brushes.insert(key, brush.clone());
                Some(brush)
            }
            Err(_) => {
                // A brush failure drops the tile, not the frame (NFR-216).
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_separates_char_format_and_colors() {
        let base = GlyphKey::new('a', 0, Color::from_hex(0x102030), Color::from_hex(0x010203));
        assert_eq!(
            base,
            GlyphKey::new('a', 0, Color::from_hex(0x102030), Color::from_hex(0x010203))
        );
        assert_ne!(
            base,
            GlyphKey::new('b', 0, Color::from_hex(0x102030), Color::from_hex(0x010203))
        );
        assert_ne!(
            base,
            GlyphKey::new('a', 1, Color::from_hex(0x102030), Color::from_hex(0x010203))
        );
        assert_ne!(
            base,
            GlyphKey::new('a', 0, Color::from_hex(0x405060), Color::from_hex(0x010203))
        );
        assert_ne!(
            base,
            GlyphKey::new('a', 0, Color::from_hex(0x102030), Color::from_hex(0x405060))
        );
    }

    #[test]
    fn shelves_pack_then_report_full() {
        let mut shelves = Shelves::new(100, 10, 20);
        assert_eq!(shelves.capacity(), 10 * 5);
        assert_eq!(shelves.next_slot(), Some((0, 0)));
        assert_eq!(shelves.next_slot(), Some((10, 0)));
        // 10 tiles per shelf, 5 shelves: slots 3..49 fill the atlas.
        for _ in 2..shelves.capacity() - 1 {
            let _ = shelves.next_slot();
        }
        assert_eq!(shelves.next_slot(), Some((90, 80)));
        // The atlas is full: the 51st tile does not fit.
        assert_eq!(shelves.next_slot(), None);
        shelves.restart();
        assert_eq!(shelves.next_slot(), Some((0, 0)));
    }

    #[test]
    fn shelves_never_report_zero_capacity() {
        let shelves = Shelves::new(10, 64, 64);
        assert_eq!(shelves.capacity(), 1);
    }
}
