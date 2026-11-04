#[cfg(test)]
mod tests {
    use crate::color::{Color, ansi_palette};
    use crate::grid::{Cell, CellAttribute, Grid};
    use crate::parser::*;

    fn grid() -> Grid {
        Grid::with_size(10, 5, 20)
    }

    fn feed(parser: &mut Parser, grid: &mut Grid, s: &str) -> Vec<ParserEvent> {
        parser.advance(grid, s.as_bytes()).events
    }

    fn rows(grid: &Grid) -> Vec<String> {
        (0..grid.rows).map(|r| grid.line_text(r).unwrap()).collect()
    }

    fn cell(grid: &Grid, row: usize, col: usize) -> Cell {
        grid.row(row).unwrap().cells[col].clone()
    }

    /// Print five labelled rows without triggering a scroll.
    fn fill(parser: &mut Parser, grid: &mut Grid) {
        feed(parser, grid, "r0\r\nr1\r\nr2\r\nr3\r\nr4");
    }

    #[test]
    fn print_text_at_cursor() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "hello");
        assert_eq!(g.line_text(0).unwrap(), "hello");
        assert_eq!((p.cursor_row, p.cursor_col), (0, 5));
    }

    #[test]
    fn cr_bs_ht_control_chars() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "abcdef\rXY\u{8}Z\tQ");
        // CR -> col 0, XY overwrites; BS backs over Y; HT -> tab stop col 8
        assert_eq!(rows(&g)[0], "XZcdef  Q");
    }

    #[test]
    fn lf_at_bottom_scrolls_into_scrollback() {
        let mut p = Parser::new();
        let mut g = grid();
        fill(&mut p, &mut g);
        let before = g.total_lines();
        feed(&mut p, &mut g, "\r\noverflow");
        assert_eq!(g.total_lines(), before + 1);
        assert_eq!(rows(&g)[4], "overflow");
        g.scroll_up(1);
        assert_eq!(rows(&g)[4], "r4");
    }

    #[test]
    fn cup_positions_cursor() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "\x1b[3;5HX");
        assert_eq!(rows(&g)[2], "    X");
        assert_eq!((p.cursor_row, p.cursor_col), (2, 5));
    }

    #[test]
    fn cursor_moves_clamp() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "\x1b[100;100H");
        assert_eq!((p.cursor_row, p.cursor_col), (4, 9));
        feed(&mut p, &mut g, "\x1b[999A\x1b[999D");
        assert_eq!((p.cursor_row, p.cursor_col), (0, 0));
    }

    #[test]
    fn cud_at_bottom_does_not_scroll() {
        // Issue #5: CUD stops at the region bottom; only LF scrolls.
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "\x1b[5;1HX\x1b[1B");
        assert_eq!(p.cursor_row, 4);
        assert_eq!(rows(&g)[4], "X");
        assert_eq!(rows(&g)[3], "");
    }

    #[test]
    fn autowrap_wraps_at_last_column() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "0123456789A");
        assert_eq!(rows(&g)[0], "0123456789");
        assert_eq!(rows(&g)[1], "A");
    }

    #[test]
    fn cup_clears_pending_wrap() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "0123456789\x1b[1;1HZ");
        assert_eq!(rows(&g)[0], "Z123456789");
        assert_eq!(rows(&g)[1], "");
    }

    #[test]
    fn ed_erases_relative_to_cursor() {
        let mut p = Parser::new();
        let mut g = grid();
        fill(&mut p, &mut g);
        feed(&mut p, &mut g, "\x1b[2;3H\x1b[J");
        // ED 0 erases from the cursor to the end of screen. "r1" lives in
        // cols 0-1, before the cursor at col 2, so it survives; rows
        // below the cursor blank.
        assert_eq!(rows(&g), ["r0", "r1", "", "", ""]);
        feed(&mut p, &mut g, "\x1b[3;3H\x1b[1J");
        assert_eq!(&rows(&g)[..2], ["", ""]);
        feed(&mut p, &mut g, "\x1b[2J");
        assert_eq!(rows(&g), ["", "", "", "", ""]);
    }

    #[test]
    fn el_erases_line() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "abcdefg");
        feed(&mut p, &mut g, "\x1b[1;3H\x1b[K");
        assert_eq!(rows(&g)[0], "ab");
        // EL 1 erases from line start through the cursor inclusive.
        // Cells 0..=1 are already blank from the EL 0 above, so this
        // clears the remaining "c" at col 2 and the line is empty.
        feed(&mut p, &mut g, "\x1b[1;3H\x1b[1K");
        assert_eq!(rows(&g)[0], "");
        feed(&mut p, &mut g, "\x1b[2K");
        assert_eq!(rows(&g)[0], "");
    }

    #[test]
    fn ich_dch_ech_edit_cells() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "abcde");
        feed(&mut p, &mut g, "\x1b[1;2H\x1b[2@");
        assert_eq!(rows(&g)[0], "a  bcde");
        feed(&mut p, &mut g, "\x1b[1;2H\x1b[2P");
        assert_eq!(rows(&g)[0], "abcde");
        feed(&mut p, &mut g, "\x1b[1;2H\x1b[2X");
        // ECH blanks in place (no shifting): from "a  cde", cells 1..=2
        // (already blank) stay blank and "c" at col 3 is erased -> "a  de".
        assert_eq!(rows(&g)[0], "a  de");
    }

    #[test]
    fn il_dl_insert_and_delete_lines() {
        let mut p = Parser::new();
        let mut g = grid();
        fill(&mut p, &mut g);
        feed(&mut p, &mut g, "\x1b[2;1H\x1b[1L");
        assert_eq!(rows(&g), ["r0", "", "r1", "r2", "r3"]);
        feed(&mut p, &mut g, "\x1b[2;1H\x1b[1M");
        assert_eq!(&rows(&g)[1..], ["r1", "r2", "r3", ""]);
    }

    #[test]
    fn decstbm_scrolls_only_the_region() {
        let mut p = Parser::new();
        let mut g = grid();
        fill(&mut p, &mut g);
        // Region = viewport rows 1..=3 (DECSTBM 2;4)
        feed(&mut p, &mut g, "\x1b[2;4r\x1b[4;1HX\r\n");
        // LF at the region bottom scrolls rows 1..=3 up: "X3" moves to
        // viewport row 2, the region's bottom blanks, rows 0 and 4 stay.
        assert_eq!(rows(&g), ["r0", "r2", "X3", "", "r4"]);
    }

    #[test]
    fn su_sd_scroll_whole_region() {
        let mut p = Parser::new();
        let mut g = grid();
        fill(&mut p, &mut g);
        feed(&mut p, &mut g, "\x1b[1S");
        assert_eq!(rows(&g), ["r1", "r2", "r3", "r4", ""]);
        feed(&mut p, &mut g, "\x1b[1T");
        assert_eq!(&rows(&g)[..2], ["", "r1"]);
    }

    #[test]
    fn esc_m_reverse_index_scrolls_at_top() {
        // Issue #6: RI moves the cursor up; only at the region top does it
        // scroll the region down.
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "top\r\nbody");
        assert_eq!(p.cursor_row, 1);
        feed(&mut p, &mut g, "\x1bM");
        assert_eq!(p.cursor_row, 0);
        assert_eq!(rows(&g), ["top", "body", "", "", ""]);
        feed(&mut p, &mut g, "\x1bM");
        assert_eq!(rows(&g), ["", "top", "body", "", ""]);
        assert_eq!(p.cursor_row, 0);
    }

    #[test]
    fn sgr_sets_pen_on_cells() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "\x1b[1;31mR\x1b[0mN");
        let red = cell(&g, 0, 0);
        assert_eq!(red.char, 'R');
        assert_eq!(red.fg, Some(ansi_palette()[1]));
        assert!(red.attributes.contains(CellAttribute::BOLD));
        let plain = cell(&g, 0, 1);
        assert_eq!(plain.fg, None);
        assert_eq!(plain.attributes, CellAttribute::empty());
    }

    #[test]
    fn sgr_256_and_truecolor() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "\x1b[38;5;196mA");
        assert_eq!(cell(&g, 0, 0).fg, Some(Color::rgb(255, 0, 0)));
        feed(&mut p, &mut g, "\x1b[38;2;10;20;30mB");
        assert_eq!(cell(&g, 0, 1).fg, Some(Color::rgb(10, 20, 30)));
        feed(&mut p, &mut g, "\x1b[48;5;22mC");
        assert_eq!(cell(&g, 0, 2).bg, Some(Color::rgb(0, 95, 0)));
        feed(&mut p, &mut g, "\x1b[38:2::1:2:3mD");
        assert_eq!(cell(&g, 0, 3).fg, Some(Color::rgb(1, 2, 3)));
    }

    #[test]
    fn sgr_clears_attributes() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "\x1b[1;2;4mA\x1b[22mB\x1b[24mC");
        assert!(cell(&g, 0, 0).attributes.contains(CellAttribute::UNDERLINE));
        let b = cell(&g, 0, 1).attributes;
        assert!(!b.contains(CellAttribute::BOLD));
        assert!(!b.contains(CellAttribute::DIM));
        assert!(b.contains(CellAttribute::UNDERLINE));
        assert!(!cell(&g, 0, 2).attributes.contains(CellAttribute::UNDERLINE));
    }

    #[test]
    fn decsc_decrc_save_restore_cursor() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "\x1b[3;4H\x1b7\x1b[1;1H\x1b8X");
        assert_eq!(rows(&g)[2], "   X");
        // CSI s / CSI u are the CSI-form save/restore (xterm).
        feed(&mut p, &mut g, "\x1b[5;5H\x1b[s\x1b[1;1H\x1b[uY");
        assert_eq!(rows(&g)[4], "    Y");
    }

    #[test]
    fn osc_title_events() {
        let mut p = Parser::new();
        let mut g = grid();
        assert_eq!(
            feed(&mut p, &mut g, "\x1b]2;vim\x07"),
            vec![ParserEvent::Title("vim".into())]
        );
        assert_eq!(
            feed(&mut p, &mut g, "\x1b]0;git\x1b\\"),
            vec![ParserEvent::Title("git".into())]
        );
        // Non-title OSCs are consumed silently.
        assert!(feed(&mut p, &mut g, "\x1b]4;1;rgb:00/00/00\x07").is_empty());
    }

    #[test]
    fn alt_screen_events_and_cursor_home() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "\x1b[3;3H");
        assert_eq!(
            feed(&mut p, &mut g, "\x1b[?1049h"),
            vec![ParserEvent::EnterAltScreen]
        );
        assert_eq!((p.cursor_row, p.cursor_col), (0, 0));
        assert_eq!(
            feed(&mut p, &mut g, "\x1b[?1049l"),
            vec![ParserEvent::ExitAltScreen]
        );
        assert_eq!((p.cursor_row, p.cursor_col), (2, 2));
    }

    #[test]
    fn cursor_visibility_event() {
        let mut p = Parser::new();
        let mut g = grid();
        assert_eq!(
            feed(&mut p, &mut g, "\x1b[?25l"),
            vec![ParserEvent::CursorVisibility(false)]
        );
        assert!(!p.cursor_visible);
        assert_eq!(
            feed(&mut p, &mut g, "\x1b[?25h"),
            vec![ParserEvent::CursorVisibility(true)]
        );
    }

    #[test]
    fn modes_tracked() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "\x1b[?2004h\x1b[?1h\x1b[?7l");
        assert!(p.bracketed_paste);
        assert!(p.application_cursor_keys);
        assert!(!p.auto_wrap);
        feed(&mut p, &mut g, "\x1b[?2004l\x1b[?1l\x1b[?7h");
        assert!(!p.bracketed_paste);
        assert!(!p.application_cursor_keys);
        assert!(p.auto_wrap);
    }

    #[test]
    fn dsr_and_da_respond() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "\x1b[3;5H\x1b[6n");
        assert_eq!(
            feed(&mut p, &mut g, "\x1b[5;1H\x1b[6n"),
            vec![ParserEvent::Respond(b"\x1b[5;1R".to_vec())]
        );
        assert_eq!(
            feed(&mut p, &mut g, "\x1b[c"),
            vec![ParserEvent::Respond(b"\x1b[?6c".to_vec())]
        );
    }

    #[test]
    fn utf8_multibyte_and_streaming_split() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "h\u{00E9}\u{1F600}");
        assert_eq!(rows(&g)[0], "h\u{00E9}\u{1F600}");
        // Split the same emoji across two advance() calls.
        let mut p2 = Parser::new();
        let mut g2 = grid();
        let bytes = "\u{1F600}".as_bytes().to_vec();
        p2.advance(&mut g2, &bytes[..2]);
        assert_eq!(g2.line_text(0).unwrap(), "");

        p2.advance(&mut g2, &bytes[2..]);
        assert_eq!(g2.line_text(0).unwrap(), "\u{1F600}");
    }

    #[test]
    fn dcs_and_apc_are_ignored() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "\x1bPq#Gjunk\x1b\\X");
        assert_eq!(rows(&g)[0], "X");
    }

    #[test]
    fn ris_resets_everything() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "\x1b[3;3H\x1b[1m\x1b[?2004h\x1bc");
        assert_eq!(p.cursor_row, 0);
        assert_eq!(p.cursor_col, 0);
        assert!(!p.bracketed_paste);
        assert!(p.cursor_visible);
    }

    #[test]
    fn c1_control_bytes_dispatch() {
        let mut p = Parser::new();
        let mut g = grid();
        // 0x9B is C1 CSI: "3 ; 4 H" must move the cursor like CSI 3;4H
        p.advance(&mut g, &[0x9B, b'3', b';', b'4', b'H']);
        p.advance(&mut g, "X".as_bytes());
        assert_eq!(rows(&g)[2], "   X");
    }

    #[test]
    fn bracketed_paste_helper() {
        assert_eq!(bracketed_paste_bytes("hi", false), b"hi");
        assert_eq!(bracketed_paste_bytes("hi", true), b"\x1b[200~hi\x1b[201~");
    }

    #[test]
    fn printed_flag_marks_visible_output() {
        let mut p = Parser::new();
        let mut g = grid();
        assert!(!p.advance(&mut g, b"\x1b[2;2H").printed);
        assert!(p.advance(&mut g, b"x").printed);
    }
    #[test]
    fn mouse_reporting_modes_and_encoding() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "[?1000h[?1006h");
        assert_eq!(p.mouse_protocol, MouseProtocol::Click);
        assert!(p.mouse_sgr_coordinates);
        feed(&mut p, &mut g, "[?1002h[?1003h");
        assert_eq!(p.mouse_protocol, MouseProtocol::Any);
        assert_eq!(
            mouse_report_bytes(0, 2, 3, false, true, false),
            b"[0;3;4M".to_vec()
        );
        assert_eq!(
            mouse_report_bytes(0, 2, 3, true, true, false),
            b"[0;3;4m".to_vec()
        );
        assert_eq!(
            mouse_report_bytes(2, 0, 0, false, false, true),
            b"[34;1;1M".to_vec()
        );
    }

    use crate::graphics::GraphicsAction;

    #[test]
    fn kitty_apc_transmit_over_st() {
        let mut p = Parser::new();
        let mut g = grid();
        let events = feed(&mut p, &mut g, "\x1b_Ga=t,i=7;YWJj\x1b\\hi");
        assert!(
            events.contains(&ParserEvent::Graphics(GraphicsAction::Transmit {
                id: 7,
                format: 100,
                width: 0,
                height: 0,
                data: b"abc".to_vec(),
                more: 0,
                display: false,
            })),
            "APC transmit becomes a Graphics event"
        );
        assert_eq!(rows(&g)[0], "hi", "text after the payload prints");
    }

    #[test]
    fn kitty_over_dcs_and_int8_dcs() {
        let mut p = Parser::new();
        let mut g = grid();
        let events = feed(&mut p, &mut g, "\x1bPGa=I\x1b\\");
        assert!(events.contains(&ParserEvent::Graphics(GraphicsAction::Query(None))));
        let events: Vec<ParserEvent> = p
            .advance(
                &mut g,
                &[0x90, b'G', b'a', b'=', b'Q', b',', b'i', b'=', b'5', 0x9C],
            )
            .events;
        assert!(events.contains(&ParserEvent::Graphics(GraphicsAction::Query(Some(5)))));
    }

    #[test]
    fn kitty_bel_terminator() {
        let mut p = Parser::new();
        let mut g = grid();
        let events = feed(&mut p, &mut g, "\x1b_Ga=d\x07");
        assert!(events.contains(&ParserEvent::Graphics(GraphicsAction::DeleteAll)));
    }

    #[test]
    fn non_g_apc_and_dcs_and_pm_stay_ignored() {
        let mut p = Parser::new();
        let mut g = grid();
        let events = feed(
            &mut p,
            &mut g,
            "\x1bPq=sx;re\x1b\\\x1b_abc\x1b\\\x1b}zz\x1b\\X",
        );
        assert!(
            !events.iter().any(|e| matches!(e, ParserEvent::Graphics(_))),
            "no graphics from other string sequences"
        );
        assert_eq!(rows(&g)[0], "X");
    }

    /// Feed all bytes the way `Pane::feed` does: resume parsing after the
    /// parser stops early on a grid-changing event (issue #1/#7).
    fn advance_all(p: &mut Parser, g: &mut Grid, bytes: &[u8]) -> Vec<ParserEvent> {
        let mut events = Vec::new();
        let mut pos = 0usize;
        while pos < bytes.len() {
            let outcome = p.advance(g, &bytes[pos..]);
            pos += outcome.consumed.min(bytes.len() - pos).max(1);
            events.extend(outcome.events);
        }
        events
    }

    /// Issue #36: portable protocol fixtures — fed whole, fed in every
    /// chunk size, and split at every byte offset — must land in the same
    /// grid, cursor and event stream. The corpus covers the wire shapes
    /// the supported apps emit: CSI, OSC with BEL/ST terminators, DCS,
    /// kitty APC/DCS, UTF-8, CJK and emoji.
    #[test]
    fn protocol_fixtures_survive_every_chunk_boundary() {
        const FIXTURES: &[&[u8]] = &[
            b"plain text\r\nwrapped lines\x1b[0m",
            b"\x1b[3;5Hpos\x1b[99;99Hclamped",
            b"\x1b[1;3rregion\x1b[H\x1b[2Jafter clear",
            b"\x1b[1mbold\x1b[31mred\x1b[0mplain",
            b"\x1b7\x1b[2;2Hsaved\x1b8rest",
            b"\x1b]0;title BEL\x07after",
            b"\x1b]2;title ST\x1b\\after",
            b"\x1b]7;file://host/path\x07cwd",
            b"\x1b]133;A\x07\x1b]133;D;0\x07prompt",
            b"\x1b[5n\x1b[c",
            b"\x1b[?1049halt\x1b[?1049lmain",
            b"\x1b[?25lhiding\x1b[?25hshown",
            b"caf\xc3\xa9 \xe4\xb8\xad\xe6\x96\x87 \xf0\x9f\x98\x80 \x07bell",
            b"\x1b_Ga=t,i=7,f=100;YWJj\x1b\\after kitty",
            b"\x1b_Ga=d\x07kitty bel",
            b"\x1bPGa=I\x1b\\dcs kitty",
            b"\x1bPq=sx;re\x1b\\\x1b_abc\x1b\\\x1b}zz\x1b\\X",
            b"del\x7eback\x08space\ttab\r\nwrap overflow",
            b"\x1b[?1000h\x1b[?1006hmouse\x1b[?1000l",
        ];
        for fixture in FIXTURES {
            let mut p = Parser::new();
            let mut g = grid();
            let whole = advance_all(&mut p, &mut g, fixture);
            let whole_rows = rows(&g);
            let whole_cursor = (p.cursor_row, p.cursor_col);

            for chunk in 1..=fixture.len() {
                let mut p = Parser::new();
                let mut g = grid();
                let mut events = Vec::new();
                for piece in fixture.chunks(chunk) {
                    events.extend(advance_all(&mut p, &mut g, piece));
                }
                assert_eq!(
                    events, whole,
                    "fixture {fixture:?} fed in {chunk}-byte chunks: events differ"
                );
                assert_eq!(
                    (p.cursor_row, p.cursor_col),
                    whole_cursor,
                    "fixture {fixture:?} fed in {chunk}-byte chunks: cursor differs"
                );
                assert_eq!(
                    rows(&g),
                    whole_rows,
                    "fixture {fixture:?} fed in {chunk}-byte chunks: grid differs"
                );
            }
            for split in 1..fixture.len() {
                let mut p = Parser::new();
                let mut g = grid();
                let mut events = advance_all(&mut p, &mut g, &fixture[..split]);
                events.extend(advance_all(&mut p, &mut g, &fixture[split..]));
                assert_eq!(
                    events, whole,
                    "fixture {fixture:?} split at byte {split}: events differ"
                );
                assert_eq!(
                    rows(&g),
                    whole_rows,
                    "fixture {fixture:?} split at byte {split}: grid differs"
                );
            }
        }
    }

    // Issue #41: OSC 8 hyperlinks. `ESC ] 8 ; params ; uri ST`.

    #[test]
    fn osc8_cells_carry_link_id() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(
            &mut p,
            &mut g,
            "\x1b]8;;https://example.com\x1b\\click\x1b]8;;\x1b\\!",
        );
        assert_eq!(g.line_text(0).unwrap(), "click!");
        for col in 0..5 {
            let id = cell(&g, 0, col).link;
            assert!(id != 0, "col {col} should carry a link id");
            assert_eq!(p.link_uri(id), Some("https://example.com"));
        }
        assert_eq!(cell(&g, 0, 5).link, 0);
    }

    #[test]
    fn osc8_params_ignored_empty_uri_closes() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(
            &mut p,
            &mut g,
            "\x1b]8;id=1;https://a\x1b\\x\x1b]8;;\x1b\\y",
        );
        let id = cell(&g, 0, 0).link;
        assert_eq!(p.link_uri(id), Some("https://a"));
        assert_eq!(cell(&g, 0, 1).link, 0);
    }

    #[test]
    fn osc8_sgr_reset_closes_link() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "\x1b]8;;https://a\x1b\\\x1b[0mx");
        assert_eq!(cell(&g, 0, 0).link, 0);
    }

    #[test]
    fn osc8_same_uri_reuses_id() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(
            &mut p,
            &mut g,
            "\x1b]8;;https://a\x1b\\one\x1b]8;;\x1b\\ \x1b]8;;https://a\x1b\\two",
        );
        assert_eq!(cell(&g, 0, 0).link, cell(&g, 0, 5).link);
    }

    #[test]
    fn osc8_open_close_events() {
        let mut p = Parser::new();
        let mut g = grid();
        let events = feed(&mut p, &mut g, "\x1b]8;;https://a\x1b\\x\x1b]8;;\x1b\\");
        assert!(events.contains(&ParserEvent::Hyperlink(Some("https://a".into()))));
        assert!(events.contains(&ParserEvent::Hyperlink(None)));
    }

    #[test]
    fn osc8_wrapped_link_spans_rows() {
        let mut p = Parser::new();
        let mut g = grid(); // width 10
        feed(&mut p, &mut g, "\x1b]8;;https://a\x1b\\0123456789ab");
        for row in 0..2 {
            for col in 0..10 {
                if row == 1 && col > 1 {
                    break;
                }
                assert_eq!(cell(&g, row, col).link, cell(&g, 0, 0).link);
            }
        }
    }

    #[test]
    fn osc8_cap_bounds_the_table() {
        let mut p = Parser::new();
        let mut g = grid();
        // Write every `x` into the same cell (CUP first) so there is no
        // wrap, no scroll, and exactly one id to read per round.
        for i in 0..(LINK_CAP + 1) {
            feed(
                &mut p,
                &mut g,
                &format!("\x1b[1;1H\x1b]8;;https://u{i}\x1b\\x\x1b]8;;\x1b\\"),
            );
            let id = g.row(0).unwrap().cells[0].link;
            if i < LINK_CAP {
                assert_eq!(
                    p.link_uri(id),
                    Some(format!("https://u{i}").as_str()),
                    "uri #{i}"
                );
            } else {
                assert_eq!(p.link_uri(id), None, "uri past the cap has no target");
            }
        }
    }

    #[test]
    fn osc8_malformed_is_harmless() {
        let mut p = Parser::new();
        let mut g = grid();
        // No URI part, and an OSC 8 inside a C1 string: text prints, no link.
        feed(&mut p, &mut g, "\x1b]8;;\x1b\\ok");
        assert_eq!(g.line_text(0).unwrap(), "ok");
        assert_eq!(cell(&g, 0, 0).link, 0);
    }
    #[test]
    fn overwriting_wide_lead_clears_continuation() {
        let mut p = Parser::new();
        let mut g = Grid::with_size(8, 2, 0);
        feed(&mut p, &mut g, "界X\rA");
        assert_eq!(g.line_text_abs(0).as_deref(), Some("A X"));
        assert!(!g.line_abs(0).unwrap().cells[1].continuation);
    }

    #[test]
    fn nonwrapping_wide_at_margin_does_not_drop_following_ascii() {
        let mut p = Parser::new();
        let mut g = Grid::with_size(4, 2, 0);
        feed(&mut p, &mut g, "\x1b[?7lab界Z");
        assert_eq!(g.line_abs(0).unwrap().cells[3].char, 'Z');
    }

    #[test]
    fn delayed_combining_marks_dirty_the_row_and_revision() {
        let mut p = Parser::new();
        let mut g = Grid::with_size(8, 2, 0);
        feed(&mut p, &mut g, "e");
        g.clear_dirty_rows();
        let rev = g.content_rev();
        feed(&mut p, &mut g, "\u{301}");
        assert!(
            !g.dirty_rows().is_empty(),
            "accent changed text without invalidating row"
        );
        assert!(g.content_rev() > rev, "search revision must change");
    }

    #[test]
    fn combining_at_right_margin_attaches_to_last_printed_cell() {
        let mut p = Parser::new();
        let mut g = Grid::with_size(3, 2, 0);
        feed(&mut p, &mut g, "abe\u{301}");
        assert_eq!(g.line_text_abs(0).as_deref(), Some("abe\u{301}"));
    }
}
