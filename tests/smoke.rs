//! End-to-end smoke test of the data model: app -> tab -> pane -> grid,
//! plus config parsing. Windowing/ConPTY/rendering are not in scope yet.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use rustty::app::App;
use rustty::config::Config;
use rustty::pane::Pane;
use rustty::shell::ShellState;

#[test]
fn fresh_app_has_one_tab_and_a_live_grid() {
    let mut app = App::new();
    assert_eq!(app.tab_count(), 1);

    let tab = app.active_tab_mut();
    assert!(tab.is_some(), "app must always have an active tab");

    // Drive the grid through the pane parser, the way shell output will.
    app.feed_active(b"smoke");

    let size = app.active_tab().map(|t| {
        let pane = t.pane();
        let g = pane.grid();
        (g.cols, g.rows)
    });
    let first_line = app.active_tab().and_then(|t| t.pane().grid().line_text(0));
    let shell = app.active_tab().map(|t| t.pane().shell().state());
    assert_eq!(size, Some((80, 24)), "new grid is 80x24");
    assert_eq!(first_line, Some("smoke".to_string()));
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
    assert_eq!(cfg.theme.name(), "one-dark");
    assert_eq!(cfg.window.cols, 80, "omitted keys get defaults");
    assert_eq!(cfg.window.rows, 24);
    assert_eq!(cfg.scrollback, 1000);
}

/// CONFIG-021: the documented example config must load, or the docs and the
/// parser drift apart without anyone noticing.
#[test]
fn the_example_config_is_valid() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/config.example.toml"))
        .expect("config.example.toml exists");
    let cfg = Config::parse(&text).expect("config.example.toml parses");
    assert_eq!(cfg.window.cols, 80);
    assert!(cfg.tab_bar.close_button);
}

/// Wide characters take two columns; combining marks attach to the base
/// cell, not to its invisible continuation column.
#[test]
fn wide_and_combining_cells_keep_their_column_widths() {
    let mut p = Pane::with_size(10, 3, 10);
    p.feed(b"\xe7\x95\x8c\xcc\x81\x78");
    let row = p.grid().row(0).unwrap();
    assert_eq!(row.cells[0].width, 2);
    assert!(row.cells[1].continuation);
    assert_eq!(p.grid().line_text(0).as_deref(), Some("界\u{0301}x"));
}
