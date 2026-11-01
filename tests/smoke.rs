//! End-to-end smoke test of the data model: app -> tab -> pane -> grid,
//! plus config parsing. Windowing/ConPTY/rendering are not in scope yet.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use rusttyterm::app::App;
use rusttyterm::config::Config;
use rusttyterm::shell::ShellState;

#[test]
fn fresh_app_has_one_running_shape_tab() {
    let mut app = App::new();
    assert_eq!(app.tab_count(), 1);

    let tab = app.active_tab_mut();
    assert!(tab.is_some(), "app must always have an active tab");

    // Drive the grid through the tab like the parser will.
    if let Some(tab) = app.active_tab_mut() {
        tab.pane_mut().grid_mut().write_line_stub(0, "smoke");
        tab.pane_mut().shell_mut().spawn();
    }

    let grid = app.active_tab().map(|t| t.pane().grid());
    let shell = app.active_tab().map(|t| t.pane().shell().state());
    assert_eq!(
        grid.map(|g| (g.cols, g.rows)),
        Some((80, 24)),
        "new grid is 80x24"
    );
    assert_eq!(
        grid.map(|g| g.line_text(0).unwrap()),
        Some("smoke".to_string())
    );
    assert_eq!(shell, Some(ShellState::Spawning));
}

#[test]
fn tabs_add_select_close_round_trip() {
    let mut app = App::new();

    app.add_tab();
    assert_eq!(app.tab_count(), 2);
    app.active_tab_mut().unwrap().set_title("Shell");
    assert_eq!(app.active_tab().map(|t| t.title()), Some("Shell"));

    assert!(app.select_tab(0));
    assert_eq!(app.active_tab().map(|t| t.title()), Some("New Tab"));

    assert!(!app.close_tab(app.active_index()));
    assert_eq!(app.tab_count(), 1);

    assert!(
        app.close_tab(app.active_index()),
        "closing the last tab reports quit"
    );
    assert_eq!(app.tab_count(), 0, "closing the last tab empties the app");
    assert!(app.active_tab().is_none());
}

#[test]
fn config_fills_missing_sections_with_defaults() {
    let cfg = Config::parse("[theme]\nname = \"One Dark Pro\"").unwrap();
    assert_eq!(cfg.theme.name, "One Dark Pro");
    assert_eq!(cfg.window.cols, 80, "omitted sections get defaults");
    assert_eq!(cfg.window.rows, 24);
    assert_eq!(cfg.scrollback.limit, 1000);
}
