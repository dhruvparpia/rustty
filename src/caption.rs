//! Title-bar strip geometry: which part of the strip a pointer is over.
//!
//! The window draws its own caption, so Windows asks where the frame is through
//! `WM_NCHITTEST`. Pure math with no Win32 dependency: the renderer draws what
//! this module computes and the window answers hit tests with it. Coordinates
//! are client DIPs.

/// Width of one caption button.
pub const BUTTON_WIDTH: f32 = 46.0;

/// Height of the strip.
pub const STRIP_HEIGHT: f32 = 36.0;

/// Empty strip kept after the tabs, so a window with many tabs can still be
/// dragged.
pub const DRAG_GAP: f32 = 46.0;

/// What a point in the window's top strip is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Ordinary client area: terminal, tabs, the `+` button.
    Client,
    /// Empty strip: drags the window, double-click maximizes.
    Caption,
    /// Minimize button.
    Min,
    /// Maximize or restore button.
    Max,
    /// Close button.
    Close,
    /// The resize band along the top edge.
    TopEdge,
}

/// Window state a hit test depends on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ctx {
    /// The window is maximized, so it has no top resize band.
    pub maximized: bool,
    /// Height of the top resize band.
    pub resize_border: f32,
    /// End of the `+` button; 0 when the strip holds no tabs.
    pub tabs_end: f32,
}

/// Classify the point (`x`, `y`) in a window `width` DIPs wide.
#[must_use]
pub fn hit_test(x: f32, y: f32, width: f32, ctx: Ctx) -> Hit {
    if y >= STRIP_HEIGHT {
        return Hit::Client;
    }
    if !ctx.maximized && y < ctx.resize_border {
        return Hit::TopEdge;
    }
    if x >= width - BUTTON_WIDTH {
        return Hit::Close;
    }
    if x >= width - 2.0 * BUTTON_WIDTH {
        return Hit::Max;
    }
    if x >= width - 3.0 * BUTTON_WIDTH {
        return Hit::Min;
    }
    if x < ctx.tabs_end {
        return Hit::Client;
    }
    Hit::Caption
}

/// The x range of a caption button in a window `width` DIPs wide.
#[must_use]
pub fn button_span(button: Hit, width: f32) -> Option<(f32, f32)> {
    let slot = match button {
        Hit::Close => 0.0,
        Hit::Max => 1.0,
        Hit::Min => 2.0,
        _ => return None,
    };
    let right = width - slot * BUTTON_WIDTH;
    Some((right - BUTTON_WIDTH, right))
}

/// Width available for tabs and the `+` button.
#[must_use]
pub fn tab_area_width(width: f32) -> f32 {
    (width - 3.0 * BUTTON_WIDTH - DRAG_GAP).max(0.0)
}

/// Height of the chrome above the grid. Tabs at the top share the caption strip.
#[must_use]
pub fn top_chrome(tabs_on_top: bool, tab_bar: f32, caption: f32) -> f32 {
    if tabs_on_top {
        tab_bar.max(caption)
    } else {
        caption
    }
}

/// Total chrome height: above the grid plus any tab bar below it.
#[must_use]
pub fn total_chrome(tabs_on_top: bool, tab_bar: f32, caption: f32) -> f32 {
    let top = top_chrome(tabs_on_top, tab_bar, caption);
    if tabs_on_top { top } else { top + tab_bar }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 1000.0;

    fn ctx() -> Ctx {
        Ctx {
            maximized: false,
            resize_border: 6.0,
            tabs_end: 300.0,
        }
    }

    #[test]
    fn buttons_are_on_the_right() {
        // Close 954..1000, Max 908..954, Min 862..908.
        for (x, want) in [
            (999.0, Hit::Close),
            (954.0, Hit::Close),
            (953.9, Hit::Max),
            (908.0, Hit::Max),
            (907.9, Hit::Min),
            (862.0, Hit::Min),
            (861.9, Hit::Caption),
        ] {
            assert_eq!(hit_test(x, 20.0, W, ctx()), want, "x {x}");
        }
    }

    #[test]
    fn top_right_corner_of_a_maximized_window_is_close() {
        // Break caught: a dead zone where throwing the pointer at the corner
        // of a maximized window misses the close button.
        let maximized = Ctx {
            maximized: true,
            ..ctx()
        };
        assert_eq!(hit_test(999.5, 0.0, W, maximized), Hit::Close);
    }

    #[test]
    fn top_edge_resizes_unless_maximized() {
        // Break caught: no way to resize from the top, or a dead band along
        // the top of a maximized window.
        assert_eq!(hit_test(500.0, 2.0, W, ctx()), Hit::TopEdge);
        let maximized = Ctx {
            maximized: true,
            ..ctx()
        };
        assert_eq!(hit_test(500.0, 2.0, W, maximized), Hit::Caption);
    }

    #[test]
    fn empty_strip_after_the_tabs_drags_the_window() {
        assert_eq!(hit_test(300.0, 20.0, W, ctx()), Hit::Caption);
        assert_eq!(hit_test(500.0, 20.0, W, ctx()), Hit::Caption);
    }

    #[test]
    fn tabs_and_plus_stay_clickable() {
        assert_eq!(hit_test(0.0, 20.0, W, ctx()), Hit::Client);
        assert_eq!(hit_test(299.0, 20.0, W, ctx()), Hit::Client);
    }

    #[test]
    fn below_the_strip_is_client() {
        assert_eq!(hit_test(500.0, 36.0, W, ctx()), Hit::Client);
        assert_eq!(hit_test(999.0, 100.0, W, ctx()), Hit::Client);
    }

    #[test]
    fn narrow_window_keeps_all_three_buttons() {
        let c = Ctx {
            tabs_end: 16.0,
            ..ctx()
        };
        assert_eq!(hit_test(199.0, 20.0, 200.0, c), Hit::Close);
        assert_eq!(hit_test(154.0, 20.0, 200.0, c), Hit::Close);
        assert_eq!(hit_test(153.0, 20.0, 200.0, c), Hit::Max);
        assert_eq!(hit_test(107.0, 20.0, 200.0, c), Hit::Min);
        assert_eq!(tab_area_width(200.0), 16.0);
        assert_eq!(tab_area_width(100.0), 0.0);
    }

    #[test]
    fn tab_area_leaves_room_for_the_buttons_and_a_drag_gap() {
        assert_eq!(tab_area_width(1000.0), 816.0);
    }

    #[test]
    fn chrome_heights_for_each_tab_bar_arrangement() {
        // Tabs on top share the strip; hidden or bottom tabs leave it empty.
        assert_eq!(top_chrome(true, 36.0, 36.0), 36.0);
        assert_eq!(top_chrome(true, 0.0, 36.0), 36.0);
        assert_eq!(top_chrome(false, 36.0, 36.0), 36.0);
        assert_eq!(total_chrome(true, 36.0, 36.0), 36.0);
        assert_eq!(total_chrome(false, 36.0, 36.0), 72.0);
        assert_eq!(total_chrome(false, 0.0, 36.0), 36.0);
        // Fullscreen and quick windows have no caption strip.
        assert_eq!(top_chrome(true, 36.0, 0.0), 36.0);
        assert_eq!(top_chrome(true, 0.0, 0.0), 0.0);
        assert_eq!(total_chrome(false, 36.0, 0.0), 36.0);
    }

    #[test]
    fn button_spans_match_the_hit_test() {
        // Break caught: telling screen readers one place and answering
        // clicks at another.
        assert_eq!(button_span(Hit::Close, W), Some((954.0, 1000.0)));
        assert_eq!(button_span(Hit::Max, W), Some((908.0, 954.0)));
        assert_eq!(button_span(Hit::Min, W), Some((862.0, 908.0)));
        assert_eq!(button_span(Hit::Caption, W), None);
        for button in [Hit::Min, Hit::Max, Hit::Close] {
            let (lo, hi) = button_span(button, W).expect("span");
            assert_eq!(hit_test(lo, 20.0, W, ctx()), button);
            assert_eq!(hit_test(hi - 0.5, 20.0, W, ctx()), button);
        }
    }
}
