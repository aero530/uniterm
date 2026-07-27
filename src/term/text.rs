//! Text preparation for the byte-oriented ASCII view.
//!
//! This is deliberately *not* a terminal emulator. ASCII mode is the raw view: it shows what
//! arrived, with control bytes made visible as caret notation (`^[`, `^C`, `^?`) so an
//! escape-heavy stream can be debugged rather than being invisible or painting tofu.
//!
//! ANSI mode goes through [`super::emu`] instead, which is a real screen. The earlier build's
//! SGR scanner — a second, partial ANSI implementation — is gone: it only understood colour
//! and discarded every cursor movement.

/// Columns between tab stops.
const TAB_WIDTH: usize = 8;

/// Render a line's bytes as displayable text.
///
/// Control bytes become caret notation, tabs expand to the next stop, and CR/LF are dropped
/// because line splitting is the buffer's job. Invalid UTF-8 is replaced rather than rejected.
// `column` is written by the final flush but only read by tab handling, which the compiler
// cannot see through the macro.
#[allow(unused_assignments)]
pub fn visible(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut pending: Vec<u8> = Vec::new();
    let mut column = 0usize;

    // Runs of printable bytes are decoded together so multi-byte UTF-8 survives.
    macro_rules! flush {
        () => {
            if !pending.is_empty() {
                let text = String::from_utf8_lossy(&pending);
                column += text.chars().count();
                out.push_str(&text);
                pending.clear();
            }
        };
    }

    for byte in bytes {
        match byte {
            b'\n' | b'\r' => {}
            b'\t' => {
                flush!();
                let advance = TAB_WIDTH - (column % TAB_WIDTH);
                out.extend(std::iter::repeat_n(' ', advance));
                column += advance;
            }
            0x00..=0x1f | 0x7f => {
                flush!();
                out.push('^');
                out.push(if *byte == 0x7f {
                    '?'
                } else {
                    (b'@' + byte) as char
                });
                column += 2;
            }
            _ => pending.push(*byte),
        }
    }
    flush!();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_text_is_unchanged() {
        assert_eq!(visible(b"hello world"), "hello world");
    }

    #[test]
    fn line_endings_are_dropped() {
        assert_eq!(visible(b"hello\r\n"), "hello");
    }

    #[test]
    fn escape_sequences_are_shown_as_caret_notation() {
        assert_eq!(visible(b"a\x1b[31mb"), "a^[[31mb");
    }

    #[test]
    fn control_bytes_map_to_their_letters() {
        assert_eq!(visible(&[0x00]), "^@");
        assert_eq!(visible(&[0x03]), "^C");
        assert_eq!(visible(&[0x1a]), "^Z");
        assert_eq!(visible(&[0x7f]), "^?");
    }

    #[test]
    fn tabs_expand_to_stops() {
        assert_eq!(visible(b"a\tb"), "a       b");
        assert_eq!(visible(b"12345678\tx"), "12345678        x");
    }

    #[test]
    fn tab_stops_account_for_caret_notation_width() {
        // "^C" occupies two columns, so the tab advances six more.
        assert_eq!(visible(b"\x03\tx"), "^C      x");
    }

    #[test]
    fn multibyte_utf8_survives() {
        assert_eq!(visible("héllo".as_bytes()), "héllo");
        assert_eq!(visible("\u{4f60}\u{597d}".as_bytes()), "\u{4f60}\u{597d}");
    }

    #[test]
    fn multibyte_chars_count_as_one_column_for_tabs() {
        // Two chars then a tab -> six spaces to reach column 8.
        assert_eq!(visible("é\u{4f60}\tx".as_bytes()), "é\u{4f60}      x");
    }

    #[test]
    fn invalid_utf8_does_not_panic() {
        let out = visible(&[0xff, 0xfe, b'a']);
        assert!(out.ends_with('a'));
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(visible(b""), "");
    }
}
