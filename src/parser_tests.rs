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
    fn cud_at_bottom_scrolls_region() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "\x1b[5;1HX\x1b[1B");
        assert_eq!(p.cursor_row, 4);
        assert_eq!(rows(&g)[3], "X");
        assert_eq!(rows(&g)[4], "");
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
    fn esc_m_reverse_index_blanks_cursor_row() {
        let mut p = Parser::new();
        let mut g = grid();
        feed(&mut p, &mut g, "top\r\nbody");
        feed(&mut p, &mut g, "\x1b[1;1H\x1bM");
        assert_eq!(rows(&g)[0], "");
        assert_eq!(rows(&g)[1], "body");
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
}
