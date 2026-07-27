//! Keyboard to byte-stream mapping for the focused terminal view.
//!
//! Replaces `asciiCodes.ts`, which was a flat key-name to byte table: no arrow keys, no
//! function keys, and no control combinations — Ctrl+C produced `"c"` (99) rather than
//! `0x03`, so no interactive remote program was usable.
//!
//! Sequences here are the xterm defaults. Application-cursor-key mode (DECCKM) and
//! bracketed paste depend on emulator state and arrive with plan task 6.

use alacritty_terminal::term::TermMode;
use eframe::egui::{Event, Key};

/// Terminal state that changes how keys are encoded.
///
/// In ANSI mode these come from the emulator; the byte-oriented modes have no emulator, so
/// they use the defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputModes {
    /// DECCKM. Arrows send `ESC O A` instead of `ESC [ A`, which is what full-screen
    /// programs enable so they can tell cursor keys from a literal escape sequence.
    pub application_cursor: bool,
    /// Pasted text is wrapped in `ESC [ 200 ~` / `ESC [ 201 ~`, so the remote end can tell
    /// a paste from typing and not run it as commands.
    pub bracketed_paste: bool,
}

impl InputModes {
    pub fn from_term_mode(mode: TermMode) -> Self {
        Self {
            application_cursor: mode.contains(TermMode::APP_CURSOR),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
        }
    }
}

/// Convert one frame's worth of egui input events into bytes to transmit.
///
/// `enter_crlf` selects whether Return sends CR+LF or CR alone. The Tauri build always
/// sent both (`PortView.svelte` followed byte 13 with byte 10), which suits line-oriented
/// serial devices but not remote shells, so it is now a per-session choice.
pub fn encode_events(events: &[Event], enter_crlf: bool, modes: InputModes) -> Vec<u8> {
    let mut out = Vec::new();

    // egui reports a printable character as `Event::Text` *and* the physical key as
    // `Event::Key`. When a control or alt modifier is held we encode from the key event,
    // so the text event for the same keystroke has to be ignored or the byte is sent twice.
    let modified_key_pressed = events.iter().any(|e| {
        matches!(
            e,
            Event::Key {
                pressed: true,
                modifiers,
                ..
            } if modifiers.ctrl || modifiers.alt || modifiers.command
        )
    });

    for event in events {
        match event {
            Event::Text(text) if !modified_key_pressed => out.extend_from_slice(text.as_bytes()),
            Event::Paste(text) => {
                if modes.bracketed_paste {
                    out.extend_from_slice(b"\x1b[200~");
                    out.extend_from_slice(text.as_bytes());
                    out.extend_from_slice(b"\x1b[201~");
                } else {
                    out.extend_from_slice(text.as_bytes());
                }
            }
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                // Ctrl+Shift+<key> is reserved for UI shortcuts such as copy and paste, so
                // it must not also be transmitted. Ctrl+C alone still sends 0x03.
                if (modifiers.ctrl || modifiers.command) && modifiers.shift {
                    continue;
                }
                if modifiers.ctrl || modifiers.command {
                    if let Some(byte) = control_byte(*key) {
                        out.push(byte);
                        continue;
                    }
                }
                // Shift+PageUp/Down scrolls the view instead of being sent.
                if modifiers.shift && matches!(key, Key::PageUp | Key::PageDown) {
                    continue;
                }
                if let Some(bytes) = cursor_key(*key, modes.application_cursor) {
                    out.extend_from_slice(bytes);
                    continue;
                }
                if let Some(bytes) = special_key(*key, enter_crlf) {
                    out.extend_from_slice(bytes);
                }
            }
            _ => {}
        }
    }

    out
}

/// Cursor and edit keys whose encoding depends on DECCKM.
fn cursor_key(key: Key, application: bool) -> Option<&'static [u8]> {
    let bytes: &'static [u8] = match (key, application) {
        (Key::ArrowUp, false) => b"\x1b[A",
        (Key::ArrowDown, false) => b"\x1b[B",
        (Key::ArrowRight, false) => b"\x1b[C",
        (Key::ArrowLeft, false) => b"\x1b[D",
        (Key::Home, false) => b"\x1b[H",
        (Key::End, false) => b"\x1b[F",

        (Key::ArrowUp, true) => b"\x1bOA",
        (Key::ArrowDown, true) => b"\x1bOB",
        (Key::ArrowRight, true) => b"\x1bOC",
        (Key::ArrowLeft, true) => b"\x1bOD",
        (Key::Home, true) => b"\x1bOH",
        (Key::End, true) => b"\x1bOF",

        _ => return None,
    };
    Some(bytes)
}

/// Ctrl+<key> to its C0 control byte.
fn control_byte(key: Key) -> Option<u8> {
    let byte = match key {
        Key::A => 0x01,
        Key::B => 0x02,
        Key::C => 0x03,
        Key::D => 0x04,
        Key::E => 0x05,
        Key::F => 0x06,
        Key::G => 0x07,
        Key::H => 0x08,
        Key::I => 0x09,
        Key::J => 0x0a,
        Key::K => 0x0b,
        Key::L => 0x0c,
        Key::M => 0x0d,
        Key::N => 0x0e,
        Key::O => 0x0f,
        Key::P => 0x10,
        Key::Q => 0x11,
        Key::R => 0x12,
        Key::S => 0x13,
        Key::T => 0x14,
        Key::U => 0x15,
        Key::V => 0x16,
        Key::W => 0x17,
        Key::X => 0x18,
        Key::Y => 0x19,
        Key::Z => 0x1a,
        Key::OpenBracket => 0x1b,
        Key::Backslash => 0x1c,
        Key::CloseBracket => 0x1d,
        Key::Space => 0x00,
        _ => return None,
    };
    Some(byte)
}

/// Keys that map to a fixed byte or escape sequence.
fn special_key(key: Key, enter_crlf: bool) -> Option<&'static [u8]> {
    let bytes: &'static [u8] = match key {
        Key::Enter => {
            if enter_crlf {
                b"\r\n"
            } else {
                b"\r"
            }
        }
        Key::Tab => b"\t",
        Key::Backspace => b"\x08",
        Key::Delete => b"\x7f",
        Key::Escape => b"\x1b",

        Key::Insert => b"\x1b[2~",
        Key::PageUp => b"\x1b[5~",
        Key::PageDown => b"\x1b[6~",

        Key::F1 => b"\x1bOP",
        Key::F2 => b"\x1bOQ",
        Key::F3 => b"\x1bOR",
        Key::F4 => b"\x1bOS",
        Key::F5 => b"\x1b[15~",
        Key::F6 => b"\x1b[17~",
        Key::F7 => b"\x1b[18~",
        Key::F8 => b"\x1b[19~",
        Key::F9 => b"\x1b[20~",
        Key::F10 => b"\x1b[21~",
        Key::F11 => b"\x1b[23~",
        Key::F12 => b"\x1b[24~",

        _ => return None,
    };
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Modifiers;

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// Encode with default (non-application) modes.
    fn encode(events: &[Event], enter_crlf: bool) -> Vec<u8> {
        encode_events(events, enter_crlf, InputModes::default())
    }

    #[test]
    fn plain_text_passes_through() {
        let events = [Event::Text("ls".into())];
        assert_eq!(encode(&events, true), b"ls");
    }

    #[test]
    fn application_cursor_mode_changes_the_arrow_prefix() {
        let modes = InputModes {
            application_cursor: true,
            ..Default::default()
        };
        let events = [key(Key::ArrowUp, Modifiers::NONE)];
        assert_eq!(encode_events(&events, true, modes), b"\x1bOA");
        // And the normal mode still uses CSI.
        assert_eq!(encode(&events, true), b"\x1b[A");
    }

    #[test]
    fn application_cursor_mode_also_covers_home_and_end() {
        let modes = InputModes {
            application_cursor: true,
            ..Default::default()
        };
        assert_eq!(
            encode_events(&[key(Key::Home, Modifiers::NONE)], true, modes),
            b"\x1bOH"
        );
        assert_eq!(
            encode_events(&[key(Key::End, Modifiers::NONE)], true, modes),
            b"\x1bOF"
        );
    }

    #[test]
    fn bracketed_paste_wraps_the_payload() {
        let modes = InputModes {
            bracketed_paste: true,
            ..Default::default()
        };
        let events = [Event::Paste("rm -rf /".into())];
        assert_eq!(
            encode_events(&events, true, modes),
            b"\x1b[200~rm -rf /\x1b[201~"
        );
    }

    #[test]
    fn paste_is_unwrapped_when_the_mode_is_off() {
        let events = [Event::Paste("echo hi".into())];
        assert_eq!(encode(&events, true), b"echo hi");
    }

    #[test]
    fn ctrl_shift_is_reserved_for_ui_shortcuts() {
        // Ctrl+Shift+C must copy, not transmit an interrupt.
        let events = [key(Key::C, Modifiers::CTRL | Modifiers::SHIFT)];
        assert!(encode(&events, true).is_empty());
        // Plain Ctrl+C still interrupts.
        assert_eq!(encode(&[key(Key::C, Modifiers::CTRL)], true), vec![0x03]);
    }

    #[test]
    fn shift_page_keys_are_reserved_for_scrolling() {
        assert!(encode(&[key(Key::PageUp, Modifiers::SHIFT)], true).is_empty());
        assert!(encode(&[key(Key::PageDown, Modifiers::SHIFT)], true).is_empty());
        // Unshifted page keys are data.
        assert_eq!(encode(&[key(Key::PageUp, Modifiers::NONE)], true), b"\x1b[5~");
    }

    #[test]
    fn input_modes_read_from_term_mode() {
        let modes = InputModes::from_term_mode(TermMode::APP_CURSOR);
        assert!(modes.application_cursor);
        assert!(!modes.bracketed_paste);

        let modes = InputModes::from_term_mode(TermMode::BRACKETED_PASTE);
        assert!(modes.bracketed_paste);
        assert!(!modes.application_cursor);
    }

    #[test]
    fn ctrl_c_sends_etx_not_the_letter() {
        // The regression that made remote shells unusable in the Tauri build.
        let events = [key(Key::C, Modifiers::CTRL), Event::Text("c".into())];
        assert_eq!(encode(&events, true), vec![0x03]);
    }

    #[test]
    fn ctrl_combinations_cover_the_c0_range() {
        assert_eq!(encode(&[key(Key::A, Modifiers::CTRL)], true), vec![0x01]);
        assert_eq!(encode(&[key(Key::Z, Modifiers::CTRL)], true), vec![0x1a]);
        assert_eq!(
            encode(&[key(Key::OpenBracket, Modifiers::CTRL)], true),
            vec![0x1b]
        );
        assert_eq!(
            encode(&[key(Key::Space, Modifiers::CTRL)], true),
            vec![0x00]
        );
    }

    #[test]
    fn arrow_keys_send_escape_sequences() {
        assert_eq!(encode(&[key(Key::ArrowUp, Modifiers::NONE)], true), b"\x1b[A");
        assert_eq!(encode(&[key(Key::ArrowDown, Modifiers::NONE)], true), b"\x1b[B");
        assert_eq!(encode(&[key(Key::ArrowRight, Modifiers::NONE)], true), b"\x1b[C");
        assert_eq!(encode(&[key(Key::ArrowLeft, Modifiers::NONE)], true), b"\x1b[D");
    }

    #[test]
    fn function_keys_send_xterm_sequences() {
        assert_eq!(encode(&[key(Key::F1, Modifiers::NONE)], true), b"\x1bOP");
        assert_eq!(encode(&[key(Key::F5, Modifiers::NONE)], true), b"\x1b[15~");
        assert_eq!(encode(&[key(Key::F12, Modifiers::NONE)], true), b"\x1b[24~");
    }

    #[test]
    fn enter_honours_the_crlf_setting() {
        assert_eq!(encode(&[key(Key::Enter, Modifiers::NONE)], true), b"\r\n");
        assert_eq!(encode(&[key(Key::Enter, Modifiers::NONE)], false), b"\r");
    }

    #[test]
    fn paste_is_forwarded() {
        let events = [Event::Paste("echo hi".into())];
        assert_eq!(encode(&events, true), b"echo hi");
    }

    #[test]
    fn key_release_sends_nothing() {
        let events = [Event::Key {
            key: Key::A,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Modifiers::NONE,
        }];
        assert!(encode(&events, true).is_empty());
    }

    #[test]
    fn unmapped_key_sends_nothing() {
        assert!(encode(&[key(Key::F20, Modifiers::NONE)], true).is_empty());
    }

    #[test]
    fn text_still_works_when_only_shift_is_held() {
        // Shift is not a suppressing modifier, or capitals would never be sent.
        let events = [key(Key::A, Modifiers::SHIFT), Event::Text("A".into())];
        assert_eq!(encode(&events, true), b"A");
    }
}
