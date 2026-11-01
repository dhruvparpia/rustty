//! End-to-end smoke test of the data model: app -> tab -> pane -> grid,
//! plus config parsing. Windowing/ConPTY/rendering are not in scope yet.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use rusttyterm::app::App;
use rusttyterm::config::Config;
use rusttyterm::shell::ShellState;

#[test]
fn fresh_app_has_one_tab_and_a_live_grid() {
    let mut app = App::new();
    assert_eq!(app.tab_count(), 1);

    let tab = app.active_tab_mut();
    assert!(tab.is_some(), "app must always have an active tab");

    // Drive the grid through the pane parser, the way shell output will.
    app.feed_active(b"smoke");

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
    assert_eq!(
        shell,
        Some(ShellState::Inactive),
        "no process is spawned by the data model alone"
    );
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
fn config_fills_missing_keys_with_defaults() {
    let cfg = Config::parse("theme = \"one-dark\"").unwrap();
    assert_eq!(cfg.theme, "one-dark");
    assert_eq!(cfg.window.cols, 80, "omitted keys get defaults");
    assert_eq!(cfg.window.rows, 24);
    assert_eq!(cfg.scrollback, 1000);
}
