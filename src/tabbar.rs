//! Tab bar strip: geometry, hit testing and title fitting (tab bar P0,
//! spec §11).
//!
//! Pure math with no Direct2D dependency: the renderer draws exactly what
//! this module computes, and the window hit-tests clicks with it, so the
//! whole tab-bar interaction is unit-testable.

/// Height of the tab bar strip, in DIPs.
pub const TAB_BAR_HEIGHT: f32 = 28.0;

/// Narrowest tab (short or empty title).
pub const TAB_MIN_WIDTH: f32 = 96.0;

/// Widest tab; longer titles are trimmed with an ellipsis.
pub const TAB_MAX_WIDTH: f32 = 200.0;

/// Horizontal room reserved inside a tab for title padding (DIPs).
pub const TAB_CHROME: f32 = 26.0;

/// Width of the `+` new-tab button (DIPs).
pub const PLUS_WIDTH: f32 = 34.0;

/// What a click on the tab bar strip landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Tab with this index.
    Tab(usize),
    /// The `+` new-tab button.
    Plus,
    /// Empty bar area.
    None,
}

/// Computed x ranges (DIPs) of the tabs, in tab order, plus the `+` button.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    /// Half-open x range of each tab.
    pub tabs: Vec<(f32, f32)>,
    /// Half-open x range of the `+` button.
    pub plus: (f32, f32),
}

/// Width of the tab carrying `title`, measured in cell advances.
#[must_use]
pub fn tab_width(title: &str, cell_width: f32) -> f32 {
    let chars = title.chars().filter(|c| !c.is_control()).count() as f32;
    (chars * cell_width + TAB_CHROME).clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH)
}

/// Lay out `titles` left to right inside a bar of `width` DIPs with tab
/// `active` visible (see [`layout_into`]).
#[must_use]
pub fn layout(titles: &[String], width: f32, cell_width: f32) -> Layout {
    layout_active(titles, width, cell_width, 0)
}

/// [`layout`] with an explicit active tab index.
#[must_use]
pub fn layout_active(titles: &[String], width: f32, cell_width: f32, active: usize) -> Layout {
    let mut out = Layout::default();
    layout_into(titles, width, cell_width, active, &mut out);
    out
}

/// Lay out `titles` inside a bar of `width` DIPs, writing into a reused
/// [`Layout`]: the tab-bar paint path calls it every frame and must not
/// allocate (NFR-113).
///
/// Issue #20 overflow policy: the `+` button is pinned to the left so it
/// stays reachable at any width. Tabs are laid out at their natural
/// widths while they fit; when they don't, all tabs shrink uniformly to
/// [`TAB_MIN_WIDTH`] and a contiguous window of that many tabs is shown,
/// always containing `active` (selecting a later tab reveals it). Tabs
/// outside the window get zero ranges, which the renderer and hit test
/// already skip.
pub fn layout_into(
    titles: &[String],
    width: f32,
    cell_width: f32,
    active: usize,
    out: &mut Layout,
) {
    out.tabs.clear();
    let width = width.max(0.0);
    out.plus = (0.0, PLUS_WIDTH.min(width));
    let avail = (width - PLUS_WIDTH).max(0.0);
    let n = titles.len();
    let natural_total: f32 = titles.iter().map(|t| tab_width(t, cell_width)).sum();
    let fits = natural_total <= avail;
    let w = if fits {
        0.0
    } else {
        TAB_MIN_WIDTH.max(avail / n.max(1) as f32)
    };
    let visible = if fits {
        n
    } else {
        ((avail / w) as usize).clamp(1, n)
    };
    let start = if visible >= n {
        0
    } else {
        active.min(n - visible)
    };
    let mut x = PLUS_WIDTH;
    for (i, title) in titles.iter().enumerate() {
        if i < start || i >= start + visible {
            out.tabs.push((0.0, 0.0));
            continue;
        }
        let tw = if fits {
            tab_width(title, cell_width)
        } else {
            w
        };
        out.tabs.push((x, (x + tw).min(width.max(x))));
        x += tw;
    }
}

/// Hit-test a pointer x (DIPs) against a laid-out bar. The caller has
/// already established the pointer is inside the bar strip.
#[must_use]
pub fn hit_test(layout: &Layout, x: f32) -> Hit {
    if let Some(i) = layout.tabs.iter().position(|&(lo, hi)| x >= lo && x < hi) {
        return Hit::Tab(i);
    }
    if x >= layout.plus.0 && x < layout.plus.1 {
        return Hit::Plus;
    }
    Hit::None
}

/// Title trimmed to the space a tab actually has for text, with an
/// ellipsis when characters had to go. `text_width` is the tab width
/// minus [`TAB_CHROME`].
#[must_use]
pub fn fit_title(title: &str, text_width: f32, cell_width: f32) -> String {
    let mut out = String::new();
    fit_title_into(title, text_width, cell_width, &mut out);
    out
}

/// [`fit_title`] writing into a reused `String` (tab-bar paint path,
/// NFR-113: no allocation per frame).
pub fn fit_title_into(title: &str, text_width: f32, cell_width: f32, out: &mut String) {
    out.clear();
    let room = (text_width / cell_width).floor().max(0.0) as usize;
    let visible = title.chars().filter(|c| !c.is_control());
    if visible.clone().count() <= room {
        out.extend(visible);
        return;
    }
    if room == 0 {
        return;
    }
    out.extend(visible.take(room - 1));
    out.push('…');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plus_is_pinned_left_then_tabs_left_to_right() {
        let titles = vec!["bash".to_string(), "pwsh".to_string()];
        let l = layout(&titles, 800.0, 10.0);
        assert_eq!(l.tabs.len(), 2);
        assert_eq!(l.plus, (0.0, PLUS_WIDTH));
        assert_eq!(l.tabs[0].0, PLUS_WIDTH);
        assert!(l.tabs[0].0 < l.tabs[0].1);
        assert_eq!(l.tabs[0].1, l.tabs[1].0);
        assert!(l.tabs[1].1 <= 800.0);
    }

    #[test]
    fn tab_widths_stay_within_their_bounds() {
        assert_eq!(tab_width("", 10.0), TAB_MIN_WIDTH);
        assert!(tab_width(&"x".repeat(400), 10.0) <= TAB_MAX_WIDTH);
        assert!(tab_width("bash", 10.0) >= TAB_MIN_WIDTH);
    }

    #[test]
    fn hit_test_finds_tabs_the_plus_and_dead_space() {
        let titles = vec!["a".to_string(), "b".to_string()];
        let l = layout(&titles, 800.0, 10.0);
        assert_eq!(hit_test(&l, l.plus.0), Hit::Plus);
        assert_eq!(hit_test(&l, l.tabs[0].0), Hit::Tab(0));
        assert_eq!(hit_test(&l, l.tabs[1].0), Hit::Tab(1));
        assert_eq!(hit_test(&l, l.tabs[1].1 + 50.0), Hit::None);
    }

    #[test]
    fn overflow_keeps_the_plus_reachable_and_reveals_the_active_tab() {
        // Issue #20 repro: 8 tabs in a 500px bar.
        let titles = vec!["bash".to_string(); 8];
        let l = layout_active(&titles, 500.0, 10.0, 7);
        assert!(l.plus.1 > l.plus.0, "the + button must stay clickable");
        assert_eq!(hit_test(&l, l.plus.0), Hit::Plus);
        let (lo, hi) = l.tabs[7];
        assert!(hi > lo, "the active tab must be visible");
        assert!(lo >= PLUS_WIDTH);
        assert!(hi <= 500.0);
        assert_eq!(hit_test(&l, lo), Hit::Tab(7));
        // Hidden tabs have zero ranges the renderer and hit test skip.
        for &(lo, hi) in l.tabs.iter().filter(|t| t.1 > t.0) {
            assert!(lo >= PLUS_WIDTH && hi <= 500.0);
        }
        // The visible tabs form one contiguous run.
        let shown: Vec<usize> = (0..8).filter(|i| l.tabs[*i].1 > l.tabs[*i].0).collect();
        assert_eq!(
            shown,
            (shown[0]..=shown[shown.len() - 1]).collect::<Vec<_>>()
        );
    }

    #[test]
    fn overflow_shrinks_tabs_uniformly_then_windows() {
        // 4 x 200px natural widths do not fit in 466px of tab room, so
        // all visible tabs share one width.
        let titles = vec!["x".repeat(40); 4];
        let l = layout_active(&titles, 500.0, 10.0, 0);
        let widths: Vec<f32> = l.tabs.iter().map(|t| t.1 - t.0).collect();
        for w in widths.iter().filter(|w| **w > 0.0) {
            assert!(*w >= TAB_MIN_WIDTH - 0.001);
            assert!(w - widths[0] < 0.001, "uniform width");
        }
    }

    #[test]
    fn tiny_bars_never_panic_and_keep_the_plus_wide_as_possible() {
        let titles = vec!["bash".to_string(); 3];
        for width in [0.0, 1.0, 20.0, 120.0, 130.0] {
            let l = layout_active(&titles, width, 10.0, 2);
            assert_eq!(l.tabs.len(), 3);
            assert!(l.plus.1 <= width.max(0.0));
            if width > PLUS_WIDTH {
                assert!(l.tabs[2].0 < l.tabs[2].1, "active revealed at {width}");
            }
        }
    }

    #[test]
    fn titles_overfit_get_an_ellipsis() {
        let long = "C:\\Users\\dhruv\\OneDrive\\Projects\\winttyghost\\src";
        let fitted = fit_title(long, 60.0, 10.0);
        assert!(fitted.chars().count() <= 6);
        assert!(fitted.ends_with('…'));
        assert_eq!(fit_title("short", 60.0, 10.0), "short");
        assert_eq!(fit_title("long text here", 0.0, 10.0), "");
    }

    #[test]
    fn the_reuse_variants_match_the_allocating_ones() {
        let titles = vec![
            "bash".to_string(),
            "C:\\Users\\dhruv\\OneDrive\\Projects\\winttyghost\\src".to_string(),
            "".to_string(),
        ];
        let want = layout(&titles, 640.0, 10.0);
        let mut reuse = Layout {
            tabs: vec![(9.0, 9.0), (9.0, 9.0), (9.0, 9.0), (9.0, 9.0)],
            plus: (9.0, 9.0),
        };
        layout_into(&titles, 640.0, 10.0, 0, &mut reuse);
        assert_eq!(reuse, want);
        assert_eq!(reuse.tabs.len(), titles.len());

        for width in [0.0, 10.0, 15.0, 16.0, 60.0, 400.0] {
            for title in [
                &titles[0].as_str(),
                &titles[1].as_str(),
                &titles[2].as_str(),
            ] {
                let mut out = String::from("stale buffer");
                fit_title_into(title, width, 10.0, &mut out);
                assert_eq!(&out, &fit_title(title, width, 10.0), "width {width}");
            }
        }
    }
}
