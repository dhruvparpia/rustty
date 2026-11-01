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

/// Lay out `titles` left to right inside a bar of `width` DIPs. Tabs past
/// the right edge keep their (clamped) ranges: the renderer trims them and
/// the hit test simply never reports a click outside the bar width.
#[must_use]
pub fn layout(titles: &[String], width: f32, cell_width: f32) -> Layout {
    let mut tabs = Vec::with_capacity(titles.len());
    let mut x = 0.0f32;
    for title in titles {
        let w = tab_width(title, cell_width);
        tabs.push((x, (x + w).min(width.max(x))));
        x += w;
    }
    let plus_start = x.min(width.max(0.0));
    Layout {
        tabs,
        plus: (
            plus_start,
            (plus_start + PLUS_WIDTH).min(width.max(plus_start)),
        ),
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
    let chars: Vec<char> = title.chars().filter(|c| !c.is_control()).collect();
    let room = (text_width / cell_width).floor().max(0.0) as usize;
    if chars.len() <= room {
        return chars.into_iter().collect();
    }
    if room == 0 {
        return String::new();
    }
    let mut out: String = chars[..room - 1].iter().collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_lay_out_left_to_right_then_the_plus_button() {
        let titles = vec!["bash".to_string(), "pwsh".to_string()];
        let l = layout(&titles, 800.0, 10.0);
        assert_eq!(l.tabs.len(), 2);
        assert!(l.tabs[0].0 < l.tabs[0].1);
        assert_eq!(l.tabs[0].1, l.tabs[1].0);
        assert!(l.plus.0 >= l.tabs[1].1);
        assert!(l.plus.1 <= 800.0);
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
        assert_eq!(hit_test(&l, 1.0), Hit::Tab(0));
        assert_eq!(hit_test(&l, l.tabs[1].0), Hit::Tab(1));
        assert_eq!(hit_test(&l, l.plus.0), Hit::Plus);
        assert_eq!(hit_test(&l, l.plus.1 + 50.0), Hit::None);
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
}
