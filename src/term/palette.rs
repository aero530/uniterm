//! Colour palette.
//!
//! `alacritty_terminal` tracks only the colours a program *overrides* at runtime via OSC 4;
//! everything else is left to the frontend. This module supplies the defaults and resolves
//! a cell's [`Color`] into an egui colour.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use eframe::egui::Color32;

/// Default foreground.
pub const FOREGROUND: Color32 = Color32::from_rgb(204, 204, 204);
/// Default background.
pub const BACKGROUND: Color32 = Color32::from_rgb(12, 12, 12);
/// Cursor colour.
pub const CURSOR: Color32 = Color32::from_rgb(220, 220, 220);
/// Selection background.
pub const SELECTION: Color32 = Color32::from_rgb(60, 80, 120);

/// The 16 base colours.
///
/// Carried over from the Tauri build's SGR table so existing output looks the same, with the
/// bright yellow entry corrected — it used to be mapped to pure green.
const BASE: [Color32; 16] = [
    Color32::from_rgb(1, 1, 1),       // 0 black
    Color32::from_rgb(222, 56, 43),   // 1 red
    Color32::from_rgb(57, 181, 74),   // 2 green
    Color32::from_rgb(255, 199, 6),   // 3 yellow
    Color32::from_rgb(0, 111, 184),   // 4 blue
    Color32::from_rgb(118, 38, 113),  // 5 magenta
    Color32::from_rgb(44, 181, 233),  // 6 cyan
    Color32::from_rgb(204, 204, 204), // 7 white
    Color32::from_rgb(128, 128, 128), // 8 bright black
    Color32::from_rgb(255, 0, 0),     // 9 bright red
    Color32::from_rgb(0, 255, 0),     // 10 bright green
    Color32::from_rgb(255, 255, 0),   // 11 bright yellow
    Color32::from_rgb(0, 0, 255),     // 12 bright blue
    Color32::from_rgb(255, 0, 255),   // 13 bright magenta
    Color32::from_rgb(0, 255, 255),   // 14 bright cyan
    Color32::from_rgb(255, 255, 255), // 15 bright white
];

/// Resolve an xterm 256-colour index to its default RGB.
pub fn indexed(index: u8) -> Color32 {
    match index {
        0..=15 => BASE[index as usize],
        16..=231 => {
            // 6x6x6 colour cube.
            let i = index - 16;
            const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
            Color32::from_rgb(
                STEPS[(i / 36) as usize],
                STEPS[((i % 36) / 6) as usize],
                STEPS[(i % 6) as usize],
            )
        }
        232..=255 => {
            // 24-step grayscale ramp.
            let v = (8 + 10 * (index as u16 - 232)).min(255) as u8;
            Color32::from_rgb(v, v, v)
        }
    }
}

/// Default colour for a named slot.
fn named(color: NamedColor) -> Color32 {
    use NamedColor::{
        Background, BrightForeground, Cursor, DimBlack, DimBlue, DimCyan, DimForeground, DimGreen,
        DimMagenta, DimRed, DimWhite, DimYellow, Foreground,
    };
    match color {
        Foreground => FOREGROUND,
        Background => BACKGROUND,
        Cursor => CURSOR,
        BrightForeground => Color32::WHITE,
        DimForeground => FOREGROUND.gamma_multiply(0.66),
        // The dim variants are the base colours darkened.
        DimBlack => BASE[0].gamma_multiply(0.66),
        DimRed => BASE[1].gamma_multiply(0.66),
        DimGreen => BASE[2].gamma_multiply(0.66),
        DimYellow => BASE[3].gamma_multiply(0.66),
        DimBlue => BASE[4].gamma_multiply(0.66),
        DimMagenta => BASE[5].gamma_multiply(0.66),
        DimCyan => BASE[6].gamma_multiply(0.66),
        DimWhite => BASE[7].gamma_multiply(0.66),
        // Everything else is one of the 16 base slots, whose discriminants are 0..=15.
        other => {
            let slot = other as usize;
            BASE.get(slot).copied().unwrap_or(FOREGROUND)
        }
    }
}

/// Resolve a cell colour, honouring any runtime override the program has set.
pub fn resolve(color: Color, overrides: &Colors) -> Color32 {
    match color {
        Color::Spec(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(index) => overrides[index as usize]
            .map(|rgb| Color32::from_rgb(rgb.r, rgb.g, rgb.b))
            .unwrap_or_else(|| indexed(index)),
        Color::Named(name) => overrides[name]
            .map(|rgb| Color32::from_rgb(rgb.r, rgb.g, rgb.b))
            .unwrap_or_else(|| named(name)),
    }
}

/// Resolve a foreground colour, applying the bold-means-bright convention.
///
/// egui has no synthetic bold for its built-in monospace face, and scaling the glyph would
/// break cell alignment in a grid. Promoting the eight base colours to their bright variants
/// is what xterm does and keeps every cell exactly one advance wide.
pub fn resolve_foreground(color: Color, overrides: &Colors, bold: bool) -> Color32 {
    if bold {
        if let Color::Named(name) = color {
            let slot = name as usize;
            if slot < 8 {
                return resolve(Color::Named(bright_of(slot)), overrides);
            }
        }
    }
    resolve(color, overrides)
}

/// The bright counterpart of one of the eight base colour slots.
fn bright_of(slot: usize) -> NamedColor {
    match slot {
        0 => NamedColor::BrightBlack,
        1 => NamedColor::BrightRed,
        2 => NamedColor::BrightGreen,
        3 => NamedColor::BrightYellow,
        4 => NamedColor::BrightBlue,
        5 => NamedColor::BrightMagenta,
        6 => NamedColor::BrightCyan,
        _ => NamedColor::BrightWhite,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_promotes_base_colours_to_bright() {
        let overrides = Colors::default();
        assert_eq!(
            resolve_foreground(Color::Named(NamedColor::Red), &overrides, true),
            BASE[9]
        );
        assert_eq!(
            resolve_foreground(Color::Named(NamedColor::Red), &overrides, false),
            BASE[1]
        );
    }

    #[test]
    fn bold_leaves_bright_and_indexed_colours_alone() {
        let overrides = Colors::default();
        assert_eq!(
            resolve_foreground(Color::Named(NamedColor::BrightRed), &overrides, true),
            BASE[9]
        );
        assert_eq!(
            resolve_foreground(Color::Indexed(200), &overrides, true),
            indexed(200)
        );
        // The default foreground is not a base slot, so bold must not recolour it.
        assert_eq!(
            resolve_foreground(Color::Named(NamedColor::Foreground), &overrides, true),
            FOREGROUND
        );
    }

    #[test]
    fn cube_endpoints() {
        assert_eq!(indexed(16), Color32::from_rgb(0, 0, 0));
        assert_eq!(indexed(231), Color32::from_rgb(255, 255, 255));
        // 196 is pure red in the cube.
        assert_eq!(indexed(196), Color32::from_rgb(255, 0, 0));
    }

    #[test]
    fn grayscale_endpoints() {
        assert_eq!(indexed(232), Color32::from_rgb(8, 8, 8));
        assert_eq!(indexed(255), Color32::from_rgb(238, 238, 238));
    }

    #[test]
    fn base_slots_pass_through() {
        for i in 0..16u8 {
            assert_eq!(indexed(i), BASE[i as usize]);
        }
    }

    #[test]
    fn bright_yellow_is_yellow() {
        // Regression: the Tauri build mapped bright yellow to pure green.
        assert_eq!(indexed(11), Color32::from_rgb(255, 255, 0));
        assert_eq!(named(NamedColor::BrightYellow), Color32::from_rgb(255, 255, 0));
    }

    #[test]
    fn named_base_colours_match_the_table() {
        assert_eq!(named(NamedColor::Red), BASE[1]);
        assert_eq!(named(NamedColor::BrightCyan), BASE[14]);
    }

    #[test]
    fn named_special_slots() {
        assert_eq!(named(NamedColor::Foreground), FOREGROUND);
        assert_eq!(named(NamedColor::Background), BACKGROUND);
    }

    #[test]
    fn resolve_prefers_runtime_overrides() {
        let mut overrides = Colors::default();
        overrides[1] = Some(alacritty_terminal::vte::ansi::Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(
            resolve(Color::Indexed(1), &overrides),
            Color32::from_rgb(1, 2, 3)
        );
        // Unset slots fall back to the defaults.
        assert_eq!(resolve(Color::Indexed(2), &overrides), BASE[2]);
    }

    #[test]
    fn resolve_spec_is_verbatim() {
        let overrides = Colors::default();
        let rgb = alacritty_terminal::vte::ansi::Rgb { r: 10, g: 20, b: 30 };
        assert_eq!(
            resolve(Color::Spec(rgb), &overrides),
            Color32::from_rgb(10, 20, 30)
        );
    }
}
