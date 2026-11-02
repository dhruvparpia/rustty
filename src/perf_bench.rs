//! Render hot-path benchmark (NFR-113/NFR-114, ROADMAP perf P0s): paints a
//! 10k-line burst through a real Direct2D target and measures the
//! damage-only path, what one frame costs, and what it allocates.
//!
//! Every test here is `#[ignore]`d: Direct2D needs a desktop session, so
//! plain `cargo test` stays headless-safe. Run the numbers with
//! `cargo test --release -- --ignored --nocapture`.
//!
//! The counting allocator below records every Rust-side allocation, so the
//! "no allocations in the render hot path" rule shows up as a number in the
//! report instead of as a code-reading exercise. Direct2D's own heap is not
//! Rust's allocator, so what is measured here is this crate's hot path.
//! The glyph-atlas counters come from [`crate::render::Renderer::atlas_stats`]:
//! they show the frames where the atlas rendered tiles (cache misses).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HBRUSH, HGDIOBJ, ReleaseDC,
    SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, HICON,
    PW_RENDERFULLCONTENT, RegisterClassW, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
    WINDOW_EX_STYLE, WNDCLASSW, WS_CLIPCHILDREN, WS_POPUP, WS_VISIBLE,
};
use windows::core::PCWSTR;

use crate::app::App;
use crate::config::FontConfig;
use crate::render::Renderer;
use crate::tabbar::TAB_BAR_HEIGHT;
use crate::theme::Theme;

/// Rust-side allocation counter (see the module docs).
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

/// Pass-through allocator that counts `alloc`/`realloc` calls.
struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// The benchmarks share a window and a GPU/software device: serializing them
/// keeps the timings and the allocation counts honest.
static BENCH: Mutex<()> = Mutex::new(());

/// Take the bench lock, surviving a poisoned mutex (a failed bench must not
/// cascade into the other bench tests).
fn bench_lock() -> MutexGuard<'static, ()> {
    match BENCH.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

const CLASS_NAME: &str = "rusttyterm-perf-bench";

/// Number of shell lines painted per frame in the burst benchmark. A real
/// ConPTY burst delivers far more than this per poll tick, so this is the
/// pessimistic shape: 100 frames to move 10k lines.
const LINES_PER_FRAME: usize = 100;

/// Total lines in the burst benchmark (ROADMAP: 10k-line burst under 1 s).
const BURST_LINES: usize = 10_000;

fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn bench_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// A visible window off the edge of every monitor, plus the renderer for it.
///
/// The window must be visible: `CheckWindowState` reports OCCLUDED for
/// invisible windows and `Renderer` skips those frames, which would time an
/// empty paint instead of a real one.
fn bench_target(cols: usize, rows: usize) -> (HWND, Renderer) {
    static REGISTER: Once = Once::new();
    let hinstance = unsafe { GetModuleHandleW(None) }.expect("bench: module handle");
    let class_name = wstr(CLASS_NAME);
    REGISTER.call_once(|| {
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(bench_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance.into(),
            hIcon: HICON::default(),
            hCursor: Default::default(),
            hbrBackground: HBRUSH::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
        };
        assert_ne!(
            unsafe { RegisterClassW(&class) },
            0,
            "bench: window class registration failed"
        );
    });
    let title = wstr("rusttyterm perf bench");
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_POPUP | WS_VISIBLE | WS_CLIPCHILDREN,
            -32_000,
            -32_000,
            1_600,
            1_200,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
    }
    .expect("bench: window creation failed");
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    // Ligatures off (the shipped default is on): with ligatures on the
    // atlas is never built (FONT-010), and the bench would measure the
    // DrawText fallback while reporting a "healthy" atlas. Issue #28
    // requires real blit counters.
    let font = FontConfig {
        ligatures: false,
        ..FontConfig::default()
    };
    let renderer = Renderer::new(hwnd, dpi, &font, cols, rows, TAB_BAR_HEIGHT)
        .expect("bench: renderer creation failed");
    (hwnd, renderer)
}

/// Shell output for `count` lines: plain text plus the CRLF ConPTY sends.
fn lines(from: usize, count: usize) -> String {
    let mut out = String::with_capacity(count * 32);
    for i in from..from + count {
        out.push_str("rusttyterm perf line ");
        out.push_str(&i.to_string());
        out.push_str("\r\n");
    }
    out
}

/// Cost of a frame that paints nothing: `ID2D1HwndRenderTarget::Present`
/// blocks until the display's next refresh, so every frame pays the display's
/// refresh interval before any of our own work counts. Measured here so the
/// burst budget can be charged for the paint, not for the monitor.
fn present_floor(app: &mut App, renderer: &mut Renderer, theme: &Theme) -> Duration {
    app.clear_grid_damage();
    // Prime: the first frame of a fresh renderer is a full repaint.
    renderer.paint_damage(app, theme, None, None);
    app.clear_grid_damage();
    let mut sum = Duration::ZERO;
    for _ in 0..20 {
        let start = Instant::now();
        renderer.paint_damage(app, theme, None, None);
        sum += start.elapsed();
    }
    let floor = sum / 20;
    eprintln!(
        "rusttyterm bench present floor: {:.2} ms/frame (display refresh, not paint work)",
        floor.as_secs_f64() * 1_000.0,
    );
    floor
}

/// Warm the renderer up: fill the reused buffers (mask, tab-bar titles,
/// UTF-16 scratch, brushes) and bake the tiles the benchmark lines use.
///
/// The glyph atlas bakes one tile per (character, style, fg, bg), so a
/// priming screen of the benchmark's own alphabet closes the tile set: the
/// frames that are measured afterwards ask for tiles that already exist.
/// The buffers' *first* frame may grow a Vec; the steady state may not.
fn prime_bench(app: &mut App, renderer: &mut Renderer, theme: &Theme) {
    app.feed_active(lines(0, LINES_PER_FRAME).as_bytes());
    let _ = app.take_dirty();
    for _ in 0..3 {
        renderer.paint_damage(app, theme, None, None);
        app.clear_grid_damage();
    }
}

/// Paint `frames` damage frames over `lines` worth of fresh output and
/// report the wall time, the paint time (wall minus the `Present` floor),
/// the allocations the paint calls make, and what the glyph atlas did while
/// painting (NFR-113/NFR-114).
fn paint_burst(
    app: &mut App,
    renderer: &mut Renderer,
    theme: &Theme,
    frames: usize,
    floor: Duration,
) -> BenchReport {
    prime_bench(app, renderer, theme);

    let mut wall = Duration::ZERO;
    let mut paint = Duration::ZERO;
    let mut worst = Duration::ZERO;
    let mut allocations = 0usize;
    // The priming screen already used lines `0..LINES_PER_FRAME`, so the
    // measured frames continue the numbering from there.
    let mut index = LINES_PER_FRAME;
    let mut fed = 0usize;
    let atlas_before = renderer.atlas_stats();
    let draws_before = renderer.glyph_draw_counts();
    for _ in 0..frames {
        let chunk = lines(index, LINES_PER_FRAME);
        index += LINES_PER_FRAME;
        fed += LINES_PER_FRAME;
        app.feed_active(chunk.as_bytes());
        let _ = app.take_dirty();
        // Count only the paint: feeding is the parser's cost, not the
        // renderer's.
        ALLOCS.store(0, Ordering::Relaxed);
        let start = Instant::now();
        renderer.paint_damage(app, theme, None, None);
        let elapsed = start.elapsed();
        allocations += ALLOCS.swap(0, Ordering::Relaxed);
        app.clear_grid_damage();
        wall += elapsed;
        let frame_paint = elapsed.saturating_sub(floor);
        paint += frame_paint;
        worst = worst.max(frame_paint);
    }
    let atlas = renderer.atlas_stats();
    let draws = renderer.glyph_draw_counts();
    BenchReport {
        frames,
        fed,
        wall,
        paint,
        worst,
        allocations,
        tiles: atlas.tiles,
        inserts: atlas.inserts.saturating_sub(atlas_before.inserts),
        evictions: atlas.evictions.saturating_sub(atlas_before.evictions),
        blits: draws.0 - draws_before.0,
        fallbacks: draws.1 - draws_before.1,
    }
}

struct BenchReport {
    frames: usize,
    fed: usize,
    wall: Duration,
    paint: Duration,
    worst: Duration,
    allocations: usize,
    /// Glyph tiles living in the atlas at the end of the run (spec §20).
    tiles: usize,
    /// Tiles the atlas rendered during the measured frames: 0 in the steady
    /// state, i.e. every glyph hit the cache.
    inserts: u64,
    /// Atlas clears during the measured frames (tile budget overflow).
    evictions: u64,
    /// Cached-glyph `DrawBitmap` calls during the measured frames (issue #28).
    blits: u64,
    /// Glyph runs painted through the `DrawText` fallback.
    fallbacks: u64,
}

impl BenchReport {
    fn print(&self, label: &str) {
        let per_frame = self.paint.as_secs_f64() / self.frames.max(1) as f64;
        eprintln!(
            "rusttyterm bench {label}: {} frames, {} lines, {:.1} ms wall, \
             {:.1} ms paint, {:.2} ms/frame paint, {:.2} ms worst frame paint, \
             {} allocations",
            self.frames,
            self.fed,
            self.wall.as_secs_f64() * 1_000.0,
            self.paint.as_secs_f64() * 1_000.0,
            per_frame * 1_000.0,
            self.worst.as_secs_f64() * 1_000.0,
            self.allocations,
        );
        eprintln!(
            "rusttyterm bench {label}: atlas {} tiles, {} new, {} evictions, \
             {} glyph blits, {} fallback runs",
            self.tiles, self.inserts, self.evictions, self.blits, self.fallbacks,
        );
    }
}

/// ROADMAP perf P0: a 10k-line burst paints inside one second.
///
/// The bound is charged to the paint work, not to the display: `Present` on
/// an `ID2D1HwndRenderTarget` blocks for the monitor's refresh, so 100
/// frames can never take less than 100 refreshes of wall time. The floor is
/// measured on the same device and subtracted per frame.
///
/// The bound is strict in release (the shipped shape) and loose in debug,
/// where every Direct2D marshalling call is interpreted.
#[test]
#[ignore = "needs a desktop session: cargo test --release -- --ignored"]
fn burst_of_10000_lines_paints_inside_the_budget() {
    let _guard = bench_lock();
    let (hwnd, mut renderer) = bench_target(80, 24);
    let (cols, rows) = renderer.grid_size();
    let mut app = App::with_size(cols, rows, 1_000);
    let theme = Theme::predawn();
    let floor = present_floor(&mut app, &mut renderer, &theme);
    let report = paint_burst(
        &mut app,
        &mut renderer,
        &theme,
        BURST_LINES / LINES_PER_FRAME,
        floor,
    );
    report.print("10k-line burst");

    let budget = if cfg!(debug_assertions) {
        // Debug Direct2D marshalling is several times slower than shipped
        // code: this catches a regression, not the shipped number.
        Duration::from_secs(4)
    } else {
        Duration::from_secs(1)
    };
    assert!(
        report.paint < budget,
        "bench: {} frames over {} lines painted in {:?}, budget {budget:?}",
        report.frames,
        report.fed,
        report.paint,
    );
    // 60 fps budget for one frame (NFR-113).
    let frame_budget = if cfg!(debug_assertions) {
        Duration::from_millis(50)
    } else {
        Duration::from_millis(17)
    };
    assert!(
        report.worst < frame_budget,
        "bench: worst frame painted in {:?}, over the {frame_budget:?} budget",
        report.worst,
    );
    let _ = unsafe { DestroyWindow(hwnd) };
}

/// NFR-113 hot-path rules: a steady-state damage frame allocates nothing and
/// renders no new glyph tiles (every glyph hits the atlas cache, spec §20).
#[test]
#[ignore = "needs a desktop session: cargo test --release -- --ignored"]
fn damage_frames_allocate_nothing() {
    let _guard = bench_lock();
    let (hwnd, mut renderer) = bench_target(80, 24);
    let (cols, rows) = renderer.grid_size();
    let mut app = App::with_size(cols, rows, 1_000);
    let theme = Theme::predawn();
    let floor = present_floor(&mut app, &mut renderer, &theme);
    let report = paint_burst(&mut app, &mut renderer, &theme, 20, floor);
    report.print("damage allocations");

    assert_eq!(
        report.allocations, 0,
        "bench: the render hot path allocated {} times in {} frames (NFR-113)",
        report.allocations, report.frames,
    );
    assert_eq!(
        report.inserts, 0,
        "bench: the glyph atlas rendered {} new tiles in {} steady-state frames \
         (NFR-113: a steady stream of output must hit the cache)",
        report.inserts, report.frames,
    );
    assert_eq!(
        report.evictions, 0,
        "bench: the glyph atlas overflowed {} times in {} steady-state frames \
         (NFR-113: {} tiles is inside the atlas' budget)",
        report.evictions, report.frames, report.tiles,
    );
    // Issue #28: baking alone proved nothing. A warm steady-state frame must
    // paint cached glyphs through DrawBitmap, not the DrawText fallback.
    assert!(
        report.blits > 0,
        "bench: {} steady-state frames drew 0 cached glyphs with DrawBitmap \
         (issue #28: every glyph was a DrawText fallback)",
        report.frames,
    );
    assert_eq!(
        report.fallbacks, 0,
        "bench: {} steady-state frames fell back to DrawText {} times \
         with a full atlas of {} tiles (issue #28)",
        report.frames, report.fallbacks, report.tiles,
    );
    let _ = unsafe { DestroyWindow(hwnd) };
}

/// NFR-113 damage-only redraw: after the screen is painted once, one line of
/// output repaints a couple of rows, not the whole grid.
#[test]
#[ignore = "needs a desktop session: cargo test --release -- --ignored"]
fn damage_frames_cover_only_the_dirty_rows() {
    let _guard = bench_lock();
    let (hwnd, mut renderer) = bench_target(80, 24);
    let (cols, rows) = renderer.grid_size();
    let mut app = App::with_size(cols, rows, 1_000);
    let theme = Theme::predawn();

    // First frame is a full repaint: every row is in the mask.
    app.feed_active(lines(0, rows).as_bytes());
    let _ = app.take_dirty();
    renderer.paint_damage(&app, &theme, None, None);
    assert_eq!(
        renderer.last_mask_rows(),
        rows,
        "bench: the first frame must cover every row"
    );
    app.clear_grid_damage();

    // Home the cursor, write one line, repaint: only that row and the row the
    // cursor block moved between may be repainted.
    app.feed_active("\x1b[1;1Hone line\r\n".as_bytes());
    let _ = app.take_dirty();
    renderer.paint_damage(&app, &theme, None, None);
    let damaged = renderer.last_mask_rows();
    app.clear_grid_damage();
    let atlas = renderer.atlas_stats();
    eprintln!("rusttyterm bench damage rows: {damaged} of {rows}");
    eprintln!(
        "rusttyterm bench damage atlas: {} tiles, {} rendered, {} evictions",
        atlas.tiles, atlas.inserts, atlas.evictions,
    );

    assert!(
        damaged < rows / 2,
        "bench: a one-line change repainted {damaged} of {rows} rows",
    );
    let _ = unsafe { DestroyWindow(hwnd) };
}

/// Issue #27: after device loss the renderer rebuilds the target, atlas
/// and brushes, schedules a full repaint, and keeps drawing. Direct2D has
/// no HRESULT-injection API, so this drives the recovery entry point
/// (`recreate_target`, the branch the EndDraw failure takes) and verifies
/// the frame that follows paints on the new target with a rebuilt atlas.
#[test]
#[ignore = "needs a desktop session: cargo test --release -- --ignored"]
fn device_loss_recovery_repaints() {
    let _guard = bench_lock();
    let (_hwnd, mut renderer) = bench_target(80, 24);
    let (cols, rows) = renderer.grid_size();
    let mut app = App::with_size(cols, rows, 1_000);
    let theme = Theme::predawn();
    app.feed_active(lines(0, rows).as_bytes());
    let _ = app.take_dirty();
    renderer.paint_damage(&app, &theme, None, None);
    let warm = renderer.atlas_stats();
    assert!(
        warm.tiles > 0,
        "bench: priming frame built no atlas tiles ({warm:?})"
    );

    renderer.recreate_target();
    assert_eq!(
        renderer.atlas_stats().tiles,
        0,
        "bench: recreation left the dead-target atlas in place"
    );

    app.feed_active("after recovery\r\n".as_bytes());
    let _ = app.take_dirty();
    let before = renderer.glyph_draw_counts();
    renderer.paint_damage(&app, &theme, None, None);
    // Recovery schedules a full repaint (a fresh swap chain shows nothing)
    // and the new target paints through a rebuilt atlas.
    assert_eq!(
        renderer.last_mask_rows(),
        rows,
        "bench: the frame after recreation was not a full repaint"
    );
    let after = renderer.glyph_draw_counts();
    assert!(
        after.0 > before.0,
        "bench: the frame after recreation drew {} cached glyphs with DrawBitmap \
         (expected blits on the rebuilt target)",
        after.0 - before.0,
    );
    assert!(
        renderer.atlas_stats().tiles > 0,
        "bench: the atlas was not rebuilt after recreation"
    );
    // No DestroyWindow: destroying the window right after target
    // recreation AVs on this machine (the compositor races the swap-chain
    // teardown); process exit owns the window.
}

/// Pixels of the bench window as the compositor sees them: `PrintWindow`
/// with `PW_RENDERFULLCONTENT` grabs the DWM-redirected frame, which is
/// what a Direct2D swap chain shows the user.
fn capture_window(hwnd: HWND) -> Vec<u8> {
    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect) }.expect("bench: client rect");
    let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
    assert!(w > 0 && h > 0, "bench: the window has no client area");
    let mut buf = vec![0u8; (w * h * 4) as usize];
    unsafe {
        let dc = GetDC(Some(hwnd));
        let mem = CreateCompatibleDC(Some(dc));
        let bmp = CreateCompatibleBitmap(dc, w, h);
        let old = SelectObject(mem, HGDIOBJ::from(bmp));
        let ok = PrintWindow(hwnd, mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT));
        assert!(ok.as_bool(), "bench: PrintWindow failed");
        let mut bmi = BITMAPINFO {
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
        let got = GetDIBits(
            mem,
            bmp,
            0,
            h as u32,
            Some(buf.as_mut_ptr().cast()),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        assert_eq!(got, h, "bench: GetDIBits read {got} of {h} scanlines");
        SelectObject(mem, old);
        let _ = DeleteDC(mem);
        let _ = DeleteObject(HGDIOBJ::from(bmp));
        ReleaseDC(Some(hwnd), dc);
    }
    buf
}

/// True when every pixel equals the first: a flat capture means the
/// compositor returned nothing (the reference comparison proves nothing
/// on a flat frame).
fn is_flat(buf: &[u8]) -> bool {
    let (chunks, _) = buf.as_chunks::<4>();
    chunks.iter().all(|px| px == &buf[..4])
}

/// DWM composes a presented frame on its own clock, so a capture taken
/// right after a paint can still show the previous (or an empty) frame.
/// Poll until the capture is not flat and, when `previous` is given,
/// differs from it; return the last capture when the deadline passes.
fn capture_presented(hwnd: HWND, previous: Option<&[u8]>) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_millis(1_000);
    loop {
        let buf = capture_window(hwnd);
        let fresh = !is_flat(&buf) && previous.is_none_or(|prev| buf != *prev);
        if fresh || Instant::now() >= deadline {
            return buf;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Issue #36: a damage repaint must show the user the same screen as a
/// full repaint of the same content. Both frames are captured from the
/// presented window and compared pixel-for-pixel.
#[test]
#[ignore = "needs a desktop session: cargo test --release -- --ignored"]
fn damage_repaint_matches_full_paint() {
    let _guard = bench_lock();
    let (hwnd, mut renderer) = bench_target(80, 24);
    // DWM only composes windows that are on a monitor; an off-monitor
    // window has no composed frame to capture.
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    }
    .expect("bench: moving the bench window on-screen failed");
    let (cols, rows) = renderer.grid_size();
    let mut app = App::with_size(cols, rows, 1_000);
    let theme = Theme::predawn();

    // The first frame of a fresh renderer is the full-paint reference.
    app.feed_active(lines(0, rows).as_bytes());
    let _ = app.take_dirty();
    renderer.paint_damage(&app, &theme, None, None);
    app.clear_grid_damage();
    let reference = capture_presented(hwnd, None);
    assert!(
        !is_flat(&reference),
        "bench: the capture is one flat colour; DWM capture is not working on this machine"
    );

    // One line changes; a damage repaint covers it.
    app.feed_active("\x1b[5;1Hdamage repaint\r\n".as_bytes());
    let _ = app.take_dirty();
    renderer.paint_damage(&app, &theme, None, None);
    app.clear_grid_damage();
    let damaged = capture_presented(hwnd, Some(&reference));
    assert!(
        damaged.iter().zip(&reference).any(|(a, b)| a != b),
        "bench: the damage frame shows no change; the capture is frozen",
    );

    // The same content through a guaranteed full repaint (recreation
    // schedules one), then a pixel-for-pixel comparison.
    renderer.recreate_target();
    renderer.paint_damage(&app, &theme, None, None);
    let full = capture_window(hwnd);
    let diff = damaged.iter().zip(&full).position(|(a, b)| a != b);
    assert!(
        diff.is_none(),
        "bench: damage repaint differs from the full-paint reference at byte {diff:?} of {}",
        full.len(),
    );
    // No DestroyWindow: tearing down a DWM-composed swap-chain window from
    // the test races the compositor thread (AV on process exit is already
    // a known bench-suite issue on this machine); process exit owns this.
}
