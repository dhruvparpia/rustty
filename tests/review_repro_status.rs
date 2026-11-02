//! Status harness for the 2026-11-01 review repros (issues #1-#8).
//! Each test is the issue's published reproduction verbatim (APIs matched to
//! current HEAD). Run `cargo test --test review_repro_status` to learn which
//! findings are fixed; keep until triage is recorded.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use rusttyterm::{app, color, pane};

#[test]
fn issue1_alt_screen_chunk_invariance() {
    let mut batched = pane::Pane::with_size(20, 3, 10);
    batched.feed(b"main");
    batched.feed(b"\x1b[?1049hALT");
    let mut split = pane::Pane::with_size(20, 3, 10);
    split.feed(b"main");
    split.feed(b"\x1b[?1049h");
    split.feed(b"ALT");
    assert_eq!(batched.grid().line_text(0), split.grid().line_text(0));
}

#[test]
fn issue2_output_while_scrolled_back_targets_live_screen() {
    let mut p = pane::Pane::with_size(10, 3, 10);
    p.feed(b"one\r\ntwo\r\nthree\r\nfour");
    p.grid_mut().scroll_up(1);
    p.feed(b"X");
    assert_eq!(p.grid().line_text(2).as_deref(), Some("fourX"));
    assert_eq!(p.grid().line_text(1).as_deref(), Some("three"));
}

#[test]
fn issue3_grow_restores_full_height_scrolling() {
    let mut p = pane::Pane::with_size(10, 3, 10);
    p.feed(b"a\r\nb\r\nc");
    let _ = p.resize(10, 5);
    p.feed(b"\r\nd\r\ne");
    assert_eq!(p.grid().line_text(4).as_deref(), Some("e"));
    assert_eq!(p.grid().line_text(0).as_deref(), Some("a"));
}

#[test]
fn issue4_truecolor_zero_red() {
    let mut p = pane::Pane::with_size(10, 3, 0);
    p.feed(b"\x1b[38;2;0;255;0mX");
    assert_eq!(
        p.grid().row(0).unwrap().cells[0].fg,
        Some(color::Color::rgb(0, 255, 0))
    );
}

#[test]
fn issue5_cursor_down_does_not_scroll() {
    let mut p = pane::Pane::with_size(10, 3, 10);
    p.feed(b"a\r\nb\r\nc");
    p.feed(b"\x1b[B");
    assert_eq!(p.grid().line_text(0).as_deref(), Some("a"));
    assert_eq!(p.grid().total_lines(), 3);
}

#[test]
fn issue6_reverse_index_moves_cursor_up() {
    let mut p = pane::Pane::with_size(10, 3, 10);
    p.feed(b"a\r\nb\r\nc");
    p.feed(b"\x1bM");
    assert_eq!(p.parser().cursor_row, 1);
    assert_eq!(p.grid().line_text(0).as_deref(), Some("a"));
}

#[test]
fn issue7_ed3_preserves_visible_screen() {
    let mut p = pane::Pane::with_size(10, 3, 10);
    p.feed(b"a\r\nb\r\nc\r\nd");
    p.feed(b"\x1b[3J");
    assert_eq!(p.grid().line_text(2).as_deref(), Some("d"));
    assert_eq!(p.grid().total_lines(), 3);
}

#[test]
fn issue8_new_tab_inherits_geometry_and_scrollback() {
    let mut a = app::App::with_size(120, 40, 5000);
    a.add_tab();
    let tab = a.active_tab().unwrap();
    let pane = tab.pane();
    let g = pane.grid();
    assert_eq!((g.cols, g.rows, g.scrollback()), (120, 40, 5000));
}
