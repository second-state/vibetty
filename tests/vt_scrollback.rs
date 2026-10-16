//! Scrollback behavior of the (patched) vt100 parser that backs the device scroll
//! (`scroll_up` / `scroll_down` on `P/control` re-render from this parser's history).

const ROWS: u16 = 30;
const COLS: u16 = 40;

fn parser() -> vt100::Parser {
    vt100::Parser::new(ROWS, COLS, 2000)
}

/// Total scrollback depth (vt100 clamps `set_scrollback` to the buffer length).
fn depth(p: &mut vt100::Parser) -> usize {
    p.screen_mut().set_scrollback(usize::MAX);
    let n = p.screen().scrollback();
    p.screen_mut().set_scrollback(0);
    n
}

fn fill(p: &mut vt100::Parser, n: usize) {
    for i in 0..n {
        p.process(format!("line {i}\r\n").as_bytes());
    }
}

#[test]
fn full_screen_scroll_goes_to_scrollback() {
    let mut p = parser();
    fill(&mut p, 100);
    assert_eq!(depth(&mut p), 100 - usize::from(ROWS) + 1);
}

#[test]
fn region_starting_at_top_goes_to_scrollback() {
    // ESC[1;24r + SU: region touches the screen top, rows leave the screen.
    let mut p = parser();
    fill(&mut p, usize::from(ROWS));
    let before = depth(&mut p);
    p.process(b"\x1b[1;24r\x1b[5S\x1b[r");
    assert_eq!(depth(&mut p), before + 5);
}

#[test]
fn region_starting_below_top_does_not_go_to_scrollback() {
    // ESC[2;24r + SU: row 0 is pinned, scrolled-off rows never leave the screen.
    // Same as xterm; documents the behavior instead of "fixing" it.
    let mut p = parser();
    fill(&mut p, usize::from(ROWS));
    let before = depth(&mut p);
    p.process(b"\x1b[2;24r\x1b[5S\x1b[r");
    assert_eq!(depth(&mut p), before);
}

#[test]
fn erase_display_does_not_go_to_scrollback() {
    // ESC[2J ESC[3J ESC[H + full redraw (what Ink does when its frame is taller than the
    // screen) replaces the screen in place: nothing is pushed, nothing is cleared.
    let mut p = parser();
    fill(&mut p, 100);
    let before = depth(&mut p);
    p.process(b"\x1b[2J\x1b[3J\x1b[H");
    fill(&mut p, usize::from(ROWS) - 1);
    assert_eq!(depth(&mut p), before);
}

#[test]
fn scrollback_is_capped_at_configured_len() {
    let mut p = vt100::Parser::new(ROWS, COLS, 50);
    fill(&mut p, 500);
    assert_eq!(depth(&mut p), 50);
}

#[test]
fn alternate_screen_has_no_scrollback() {
    // What Claude Code (2.1.x) actually emits: alt screen + ESC[2;23r + ESC[nS.
    // vt100 gives the alternate grid a scrollback length of 0, so nothing is ever kept
    // locally; vibetty forwards PageUp/PageDown to the app instead (ws::alt_screen_scroll_key).
    let mut p = parser();
    p.process(b"\x1b[?1049h");
    fill(&mut p, 100);
    p.process(b"\x1b[2;23r\x1b[4S\x1b[1;23r\x1b[4S\x1b[r");
    assert!(p.screen().alternate_screen());
    assert_eq!(depth(&mut p), 0);
}
