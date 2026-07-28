//! Terminal emulator.
//!
//! Wraps `alacritty_terminal` — the same core Alacritty ships — so ANSI mode is a real
//! screen rather than styled text: cursor addressing, erase, scroll regions, the alternate
//! buffer, wide characters and reflow on resize all come from the emulator.
//!
//! This replaces the SGR-only scanner the earlier build used, which parsed colour escapes and
//! silently discarded every cursor movement.
//!
//! The raw byte ring in [`super::TermBuffer`] stays the source of truth. The emulator is a
//! *derived* view, fed forward from the ring, so switching display modes or changing the
//! scrollback limit can rebuild it by replaying bytes. An emulator cannot be handed the tail
//! of a stream and produce a correct screen, which is why the ring has to exist.

use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use tracing::warn;

/// Smallest usable terminal.
const MIN_COLUMNS: usize = 2;
const MIN_LINES: usize = 1;
/// Bounds on the derived scrollback, in lines.
const MIN_HISTORY: usize = 100;
const MAX_HISTORY: usize = 100_000;
/// Bytes per line assumed when converting the scrollback budget into lines.
const ASSUMED_LINE_LENGTH: usize = 80;

/// Terminal dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermSize {
    pub columns: usize,
    pub screen_lines: usize,
    pub history: usize,
}

impl TermSize {
    pub fn new(columns: usize, screen_lines: usize, history: usize) -> Self {
        Self {
            columns: columns.max(MIN_COLUMNS),
            screen_lines: screen_lines.max(MIN_LINES),
            history,
        }
    }
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.screen_lines + self.history
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// Collects the terminal's outbound replies.
///
/// Programs query the terminal (device attributes, cursor position, text-area size) and
/// expect an answer on the same channel. Dropping those makes some programs hang waiting,
/// so they are queued here and transmitted by the session.
#[derive(Clone, Default)]
pub struct Replies {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl EventListener for Replies {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(text) = event {
            if let Ok(mut bytes) = self.bytes.lock() {
                bytes.extend_from_slice(text.as_bytes());
            }
        }
    }
}

/// Convert a scrollback byte budget into a line count for the emulator.
///
/// The user's scrollback slider is in bytes because the byte-oriented views need it; the
/// emulator counts lines. This is an estimate, and the two limits are independent — see the
/// note in the README about ANSI mode holding both.
pub fn history_lines(max_bytes: usize) -> usize {
    (max_bytes / ASSUMED_LINE_LENGTH).clamp(MIN_HISTORY, MAX_HISTORY)
}

/// A live terminal screen.
pub struct Emulator {
    term: Term<Replies>,
    parser: Processor,
    replies: Replies,
    size: TermSize,
    /// Absolute offset in the byte ring up to which bytes have been fed.
    fed_to: u64,
}

impl Emulator {
    pub fn new(size: TermSize) -> Self {
        let config = Config {
            scrolling_history: size.history,
            ..Config::default()
        };
        let replies = Replies::default();
        let term = Term::new(config, &size, replies.clone());
        Self {
            term,
            parser: Processor::new(),
            replies,
            size,
            fed_to: 0,
        }
    }

    pub fn size(&self) -> TermSize {
        self.size
    }

    pub fn fed_to(&self) -> u64 {
        self.fed_to
    }

    /// Feed received bytes into the screen.
    pub fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.parser.advance(&mut self.term, bytes);
        self.fed_to += bytes.len() as u64;
    }

    /// Advance to `absolute` having skipped bytes that were trimmed before being fed.
    pub fn skip_to(&mut self, absolute: u64) {
        if absolute > self.fed_to {
            warn!(
                "terminal fell behind the byte ring; {} bytes were dropped from the screen",
                absolute - self.fed_to
            );
            self.fed_to = absolute;
        }
    }

    /// Resize the screen, reflowing existing content.
    pub fn resize(&mut self, columns: usize, screen_lines: usize) {
        let size = TermSize::new(columns, screen_lines, self.size.history);
        if size == self.size {
            return;
        }
        self.size = size;
        self.term.resize(size);
    }

    /// Replace the scrollback limit. Requires a rebuild, so the caller replays the ring.
    pub fn set_history(&mut self, history: usize) {
        self.size.history = history;
        let config = Config {
            scrolling_history: history,
            ..Config::default()
        };
        self.term.set_options(config);
    }

    /// Take any replies the terminal wants to transmit.
    pub fn take_replies(&mut self) -> Vec<u8> {
        match self.replies.bytes.lock() {
            Ok(mut bytes) => std::mem::take(&mut *bytes),
            Err(_) => Vec::new(),
        }
    }

    pub fn term(&self) -> &Term<Replies> {
        &self.term
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    /// Rows of scrollback above the viewport.
    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    /// How far the view is scrolled back, in lines.
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Scroll the viewport. Positive scrolls towards older output.
    pub fn scroll_lines(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
    }

    pub fn scroll_to(&mut self, offset_from_bottom: usize) {
        let current = self.display_offset() as i32;
        let target = offset_from_bottom as i32;
        if target != current {
            self.term.scroll_display(Scroll::Delta(target - current));
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn page(&mut self, up: bool) {
        self.term
            .scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
    }

    /// Wipe the screen and scrollback.
    pub fn reset(&mut self) {
        let fed_to = self.fed_to;
        *self = Self::new(self.size);
        self.fed_to = fed_to;
    }

    // ---- selection ----

    pub fn start_selection(&mut self, point: Point, side: Side, block: bool) {
        let ty = if block {
            SelectionType::Block
        } else {
            SelectionType::Simple
        };
        self.term.selection = Some(Selection::new(ty, point, side));
    }

    pub fn update_selection(&mut self, point: Point, side: Side) {
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side);
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub fn has_selection(&self) -> bool {
        self.term
            .selection
            .as_ref()
            .is_some_and(|s| !s.is_empty())
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|s| !s.is_empty())
    }

    /// Everything in the grid, scrollback included, as text.
    pub fn all_text(&self) -> String {
        let grid = self.term.grid();
        let top = Point::new(grid.topmost_line(), Column(0));
        let bottom = Point::new(
            Line(grid.screen_lines() as i32 - 1),
            Column(grid.columns().saturating_sub(1)),
        );
        self.term.bounds_to_string(top, bottom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::term::cell::Flags;

    fn emulator(columns: usize, lines: usize) -> Emulator {
        Emulator::new(TermSize::new(columns, lines, 200))
    }

    /// Visible rows, top to bottom, trailing blanks trimmed.
    fn screen(emu: &Emulator) -> Vec<String> {
        let content = emu.term().renderable_content();
        let offset = content.display_offset as i32;
        let lines = emu.term().grid().screen_lines();
        let mut rows = vec![String::new(); lines];
        for indexed in content.display_iter {
            if indexed.cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let row = indexed.point.line.0 + offset;
            if row >= 0 && (row as usize) < rows.len() {
                rows[row as usize].push(indexed.cell.c);
            }
        }
        rows.iter().map(|r| r.trim_end().to_string()).collect()
    }

    #[test]
    fn plain_text_lands_on_the_screen() {
        let mut emu = emulator(20, 4);
        emu.feed(b"hello\r\nworld\r\n");
        assert_eq!(screen(&emu)[0], "hello");
        assert_eq!(screen(&emu)[1], "world");
    }

    #[test]
    fn cursor_addressing_is_honoured() {
        // The whole point of task 6: the old renderer parsed and discarded this.
        let mut emu = emulator(20, 4);
        emu.feed(b"hello\r\nworld\r\n");
        emu.feed(b"\x1b[1;1HJ");
        assert_eq!(screen(&emu)[0], "Jello");
    }

    #[test]
    fn erase_in_display_clears_the_screen() {
        let mut emu = emulator(20, 4);
        emu.feed(b"hello\r\nworld");
        emu.feed(b"\x1b[2J");
        assert_eq!(screen(&emu)[0], "");
        assert_eq!(screen(&emu)[1], "");
    }

    #[test]
    fn erase_in_line_clears_to_the_right() {
        let mut emu = emulator(20, 4);
        emu.feed(b"abcdef\x1b[1;4H\x1b[K");
        assert_eq!(screen(&emu)[0], "abc");
    }

    #[test]
    fn carriage_return_overwrites_in_place() {
        let mut emu = emulator(20, 2);
        emu.feed(b"progress 10%\rprogress 99%");
        assert_eq!(screen(&emu)[0], "progress 99%");
    }

    #[test]
    fn backspace_moves_the_cursor_back() {
        let mut emu = emulator(20, 2);
        emu.feed(b"abcX\x08Y");
        assert_eq!(screen(&emu)[0], "abcY");
    }

    #[test]
    fn long_lines_wrap_at_the_column_boundary() {
        let mut emu = emulator(5, 4);
        emu.feed(b"abcdefghij");
        let rows = screen(&emu);
        assert_eq!(rows[0], "abcde");
        assert_eq!(rows[1], "fghij");
    }

    #[test]
    fn scrolling_pushes_lines_into_history() {
        let mut emu = emulator(10, 2);
        for i in 1..=6 {
            emu.feed(format!("l{i}\r\n").as_bytes());
        }
        assert!(emu.history_size() > 0);
        // Viewport shows the newest content.
        assert_eq!(screen(&emu)[0], "l6");
    }

    #[test]
    fn scrollback_can_be_scrolled_and_restored() {
        let mut emu = emulator(10, 2);
        for i in 1..=6 {
            emu.feed(format!("l{i}\r\n").as_bytes());
        }
        emu.scroll_lines(2);
        assert_eq!(emu.display_offset(), 2);
        assert_eq!(screen(&emu)[0], "l4");
        emu.scroll_to_bottom();
        assert_eq!(emu.display_offset(), 0);
        assert_eq!(screen(&emu)[0], "l6");
    }

    #[test]
    fn scroll_region_limits_scrolling() {
        let mut emu = emulator(10, 4);
        emu.feed(b"\x1b[2J\x1b[H");
        for i in 1..=4 {
            emu.feed(format!("r{i}\r\n").as_bytes());
        }
        // Restrict to lines 1-2, then scroll up twice: only those lines move.
        emu.feed(b"\x1b[1;2r\x1b[H\x1b[2S");
        let rows = screen(&emu);
        assert_eq!(rows[0], "");
        assert_eq!(rows[1], "");
        assert_eq!(rows[2], "r4", "content below the region must be untouched");
    }

    #[test]
    fn alternate_screen_is_entered_and_left() {
        let mut emu = emulator(10, 2);
        emu.feed(b"main\r\n");
        emu.feed(b"\x1b[?1049h");
        assert!(emu.mode().contains(TermMode::ALT_SCREEN));
        // 1049h saves the cursor and clears the alt screen but does not home the cursor, so
        // full-screen programs home it themselves.
        emu.feed(b"\x1b[Halt");
        assert_eq!(screen(&emu)[0], "alt");
        emu.feed(b"\x1b[?1049l");
        assert!(!emu.mode().contains(TermMode::ALT_SCREEN));
        assert_eq!(screen(&emu)[0], "main", "main screen is restored");
    }

    #[test]
    fn application_cursor_mode_is_reported() {
        let mut emu = emulator(10, 2);
        assert!(!emu.mode().contains(TermMode::APP_CURSOR));
        emu.feed(b"\x1b[?1h");
        assert!(emu.mode().contains(TermMode::APP_CURSOR));
        emu.feed(b"\x1b[?1l");
        assert!(!emu.mode().contains(TermMode::APP_CURSOR));
    }

    #[test]
    fn bracketed_paste_mode_is_reported() {
        let mut emu = emulator(10, 2);
        assert!(!emu.mode().contains(TermMode::BRACKETED_PASTE));
        emu.feed(b"\x1b[?2004h");
        assert!(emu.mode().contains(TermMode::BRACKETED_PASTE));
    }

    #[test]
    fn device_attribute_query_produces_a_reply() {
        let mut emu = emulator(10, 2);
        emu.feed(b"\x1b[c");
        let reply = emu.take_replies();
        assert!(!reply.is_empty(), "a DA query must be answered");
        assert!(reply.starts_with(b"\x1b["));
        // Draining twice does not repeat it.
        assert!(emu.take_replies().is_empty());
    }

    #[test]
    fn cursor_position_query_is_answered() {
        let mut emu = emulator(10, 4);
        emu.feed(b"\x1b[2;3H\x1b[6n");
        let reply = String::from_utf8(emu.take_replies()).unwrap();
        assert_eq!(reply, "\x1b[2;3R");
    }

    #[test]
    fn wide_characters_occupy_two_cells() {
        let mut emu = emulator(10, 2);
        emu.feed("\u{4f60}X".as_bytes());
        let content = emu.term().renderable_content();
        let row: Vec<_> = content
            .display_iter
            .filter(|i| i.point.line.0 == 0)
            .take(3)
            .map(|i| (i.cell.c, i.cell.flags))
            .collect();
        assert_eq!(row[0].0, '\u{4f60}');
        assert!(row[0].1.contains(Flags::WIDE_CHAR));
        assert!(row[1].1.contains(Flags::WIDE_CHAR_SPACER));
        assert_eq!(row[2].0, 'X');
    }

    #[test]
    fn resize_reflows_and_reports_new_size() {
        let mut emu = emulator(5, 4);
        emu.feed(b"abcdefghij");
        emu.resize(10, 4);
        assert_eq!(emu.size().columns, 10);
        assert_eq!(screen(&emu)[0], "abcdefghij", "content reflows into the wider screen");
    }

    #[test]
    fn resize_clamps_to_a_usable_minimum() {
        let mut emu = emulator(10, 4);
        emu.resize(0, 0);
        assert!(emu.size().columns >= MIN_COLUMNS);
        assert!(emu.size().screen_lines >= MIN_LINES);
    }

    #[test]
    fn feed_tracks_the_absolute_offset() {
        let mut emu = emulator(10, 2);
        emu.feed(b"12345");
        assert_eq!(emu.fed_to(), 5);
        emu.feed(b"678");
        assert_eq!(emu.fed_to(), 8);
    }

    #[test]
    fn skip_to_jumps_past_dropped_bytes() {
        let mut emu = emulator(10, 2);
        emu.feed(b"abc");
        emu.skip_to(100);
        assert_eq!(emu.fed_to(), 100);
        // Never rewinds.
        emu.skip_to(50);
        assert_eq!(emu.fed_to(), 100);
    }

    #[test]
    fn selection_yields_text() {
        let mut emu = emulator(20, 2);
        emu.feed(b"hello world");
        emu.start_selection(Point::new(Line(0), Column(0)), Side::Left, false);
        emu.update_selection(Point::new(Line(0), Column(4)), Side::Right);
        assert!(emu.has_selection());
        assert_eq!(emu.selection_text().as_deref(), Some("hello"));
        emu.clear_selection();
        assert!(!emu.has_selection());
        assert_eq!(emu.selection_text(), None);
    }

    #[test]
    fn all_text_includes_scrollback() {
        let mut emu = emulator(10, 2);
        for i in 1..=6 {
            emu.feed(format!("l{i}\r\n").as_bytes());
        }
        let all = emu.all_text();
        assert!(all.contains("l1"), "scrolled-off content must be included");
        assert!(all.contains("l6"));
    }

    #[test]
    fn reset_clears_content_but_keeps_the_feed_position() {
        let mut emu = emulator(10, 2);
        emu.feed(b"hello");
        let fed = emu.fed_to();
        emu.reset();
        assert_eq!(screen(&emu)[0], "");
        assert_eq!(emu.fed_to(), fed, "resetting must not replay the whole ring");
    }

    #[test]
    fn escape_sequences_split_across_feeds_still_work() {
        // The parser is stateful, so a sequence arriving in two reads must survive.
        let mut emu = emulator(20, 2);
        emu.feed(b"\x1b[");
        emu.feed(b"1;1HX");
        assert_eq!(screen(&emu)[0], "X");
    }

    #[test]
    fn history_lines_scales_with_the_byte_budget() {
        assert_eq!(history_lines(0), MIN_HISTORY);
        assert_eq!(history_lines(80_000), 1_000);
        assert_eq!(history_lines(usize::MAX), MAX_HISTORY);
    }

    #[test]
    fn osc_colour_override_is_applied() {
        let mut emu = emulator(10, 2);
        // OSC 4: set palette entry 1 to pure blue.
        emu.feed(b"\x1b]4;1;rgb:0000/0000/ffff\x1b\\");
        let content = emu.term().renderable_content();
        assert_eq!(
            content.colors[1],
            Some(alacritty_terminal::vte::ansi::Rgb { r: 0, g: 0, b: 255 })
        );
    }
}
