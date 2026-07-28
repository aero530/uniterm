//! Colour palette.
//!
//! `alacritty_terminal` tracks only the colours a program *overrides* at runtime via OSC 4;
//! everything else is left to the frontend. This module supplies the defaults and resolves
//! a cell's [`Color`] into an egui colour.
//!
//! Only the *defaults* are themed - foreground, background, cursor, selection. The sixteen
//! ANSI slots and the 256-colour cube are not: a program asking for slot 7 means white, and
//! quietly substituting a dark grey on the light theme would break the common "white
//! background, black text" status bar just as surely as leaving it alone breaks white text on
//! a white background.
//!
//! What makes both work is [`Palette::readable`], applied per cell once the foreground and
//! background are known. It is the same idea as Windows Terminal's
//! `adjustIndistinguishableColors`, and it fixes the dark theme too: black text on the
//! near-black default background used to be completely invisible.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use eframe::egui::Color32;

/// How far a "dim" colour is moved towards the background.
const DIM_FACTOR: f32 = 0.34;

/// Contrast ratio below which a cell's foreground is nudged away from its background.
///
/// Not a readability target - WCAG AA for body text is 4.5, and forcing that would repaint
/// most of a normal colour scheme. This is a visibility floor, and it is deliberately set as
/// low as it can be while still doing its job: at 2.0 the only dark-theme colour it changes
/// is ANSI black, which is invisible on the near-black background, and it leaves dark blue
/// and magenta (2.24 and 2.09) exactly as they have always rendered. Raising it to 2.5 would
/// restyle those two for no reason anybody asked for. On the light theme it catches the six
/// pale slots that white would otherwise swallow.
const MIN_CONTRAST: f32 = 2.0;

/// The 16 base colours.
///
/// Carried over from the Tauri build's SGR table so existing output looks the same, with the
/// bright yellow entry corrected - it used to be mapped to pure green. Not themed: see the
/// module comment.
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
            // 24-step greyscale ramp.
            let v = (8 + 10 * (index as u16 - 232)).min(255) as u8;
            Color32::from_rgb(v, v, v)
        }
    }
}

/// Relative luminance, per WCAG.
fn luminance(c: Color32) -> f32 {
    let f = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

/// WCAG contrast ratio between two colours. 1.0 is identical, 21.0 is black on white.
fn contrast(a: Color32, b: Color32) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

/// The colours a terminal screen is drawn with.
///
/// Only the defaults live here; see the module comment for why the ANSI slots do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub foreground: Color32,
    pub background: Color32,
    pub cursor: Color32,
    /// Selection background. The text keeps its own colour, so this has to stay readable.
    pub selection: Color32,
}

/// Defaults for the dark theme. Unchanged from before the light theme existed.
pub const DARK: Palette = Palette {
    foreground: Color32::from_rgb(204, 204, 204),
    background: Color32::from_rgb(12, 12, 12),
    cursor: Color32::from_rgb(220, 220, 220),
    selection: Color32::from_rgb(60, 80, 120),
};

/// Defaults for the light theme.
pub const LIGHT: Palette = Palette {
    foreground: Color32::from_rgb(31, 31, 31),
    background: Color32::from_rgb(255, 255, 255),
    cursor: Color32::from_rgb(40, 40, 40),
    selection: Color32::from_rgb(181, 213, 249),
};

impl Palette {
    /// The palette matching the application theme.
    pub fn for_theme(dark_mode: bool) -> Self {
        if dark_mode {
            DARK
        } else {
            LIGHT
        }
    }

    /// Nudge a foreground colour until it is visible against the background it sits on.
    ///
    /// Moves towards whichever extreme is further from the background - white on a dark
    /// background, black on a light one - which preserves hue far better than jumping
    /// straight to the default foreground. Returns `fg` untouched when it is already
    /// legible, which is the overwhelmingly common case.
    pub fn readable(&self, fg: Color32, bg: Color32) -> Color32 {
        if contrast(fg, bg) >= MIN_CONTRAST {
            return fg;
        }
        let target = if luminance(bg) > 0.18 {
            Color32::BLACK
        } else {
            Color32::WHITE
        };
        // Sixteen steps is finer than the eye can tell apart and always terminates: the last
        // step is `target` itself, which was chosen for having maximum contrast here.
        let mut best = fg;
        for step in 1..=16 {
            best = fg.lerp_to_gamma(target, step as f32 / 16.0);
            if contrast(best, bg) >= MIN_CONTRAST {
                break;
            }
        }
        best
    }

    /// Lower a colour's contrast against the background.
    ///
    /// Dim means "less visible", which is a *darkening* on a dark background and a
    /// *lightening* on a light one — so this moves towards the background rather than towards
    /// black. Multiplying the brightness, as this used to, would make dim text on the light
    /// theme render bolder than normal text.
    pub fn dim(&self, color: Color32) -> Color32 {
        color.lerp_to_gamma(self.background, DIM_FACTOR)
    }

    /// Default colour for a named slot.
    fn named(&self, color: NamedColor) -> Color32 {
        use NamedColor::{
            Background, BrightForeground, Cursor, DimBlack, DimBlue, DimCyan, DimForeground,
            DimGreen, DimMagenta, DimRed, DimWhite, DimYellow, Foreground,
        };
        match color {
            Foreground => self.foreground,
            Background => self.background,
            Cursor => self.cursor,
            // The most emphatic foreground available. Left unthemed like the rest of the
            // slots; the contrast floor deals with it on a light background.
            BrightForeground => BASE[15],
            DimForeground => self.dim(self.foreground),
            DimBlack => self.dim(BASE[0]),
            DimRed => self.dim(BASE[1]),
            DimGreen => self.dim(BASE[2]),
            DimYellow => self.dim(BASE[3]),
            DimBlue => self.dim(BASE[4]),
            DimMagenta => self.dim(BASE[5]),
            DimCyan => self.dim(BASE[6]),
            DimWhite => self.dim(BASE[7]),
            // Everything else is one of the 16 base slots, whose discriminants are 0..=15.
            other => {
                let slot = other as usize;
                BASE.get(slot).copied().unwrap_or(self.foreground)
            }
        }
    }

    /// Resolve a cell colour, honouring any runtime override the program has set.
    pub fn resolve(&self, color: Color, overrides: &Colors) -> Color32 {
        match color {
            Color::Spec(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
            Color::Indexed(index) => overrides[index as usize]
                .map(|rgb| Color32::from_rgb(rgb.r, rgb.g, rgb.b))
                .unwrap_or_else(|| indexed(index)),
            Color::Named(name) => overrides[name]
                .map(|rgb| Color32::from_rgb(rgb.r, rgb.g, rgb.b))
                .unwrap_or_else(|| self.named(name)),
        }
    }

    /// Resolve a foreground colour, applying the bold-means-bright convention.
    ///
    /// egui has no synthetic bold for its built-in monospace face, and scaling the glyph would
    /// break cell alignment in a grid. Promoting the eight base colours to their bright
    /// variants is what xterm does and keeps every cell exactly one advance wide.
    pub fn resolve_foreground(&self, color: Color, overrides: &Colors, bold: bool) -> Color32 {
        if bold {
            if let Color::Named(name) = color {
                let slot = name as usize;
                if slot < 8 {
                    return self.resolve(Color::Named(bright_of(slot)), overrides);
                }
            }
        }
        self.resolve(color, overrides)
    }
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
            DARK.resolve_foreground(Color::Named(NamedColor::Red), &overrides, true),
            BASE[9]
        );
        assert_eq!(
            DARK.resolve_foreground(Color::Named(NamedColor::Red), &overrides, false),
            BASE[1]
        );
    }

    #[test]
    fn bold_leaves_bright_and_indexed_colours_alone() {
        let overrides = Colors::default();
        assert_eq!(
            DARK.resolve_foreground(Color::Named(NamedColor::BrightRed), &overrides, true),
            BASE[9]
        );
        assert_eq!(
            DARK.resolve_foreground(Color::Indexed(200), &overrides, true),
            indexed(200)
        );
        // The default foreground is not a base slot, so bold must not recolour it.
        assert_eq!(
            DARK.resolve_foreground(Color::Named(NamedColor::Foreground), &overrides, true),
            DARK.foreground
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
        assert_eq!(
            DARK.named(NamedColor::BrightYellow),
            Color32::from_rgb(255, 255, 0)
        );
    }

    #[test]
    fn named_base_colours_match_the_table() {
        assert_eq!(DARK.named(NamedColor::Red), BASE[1]);
        assert_eq!(DARK.named(NamedColor::BrightCyan), BASE[14]);
    }

    #[test]
    fn named_special_slots() {
        assert_eq!(DARK.named(NamedColor::Foreground), DARK.foreground);
        assert_eq!(DARK.named(NamedColor::Background), DARK.background);
        assert_eq!(LIGHT.named(NamedColor::Foreground), LIGHT.foreground);
        assert_eq!(LIGHT.named(NamedColor::Background), LIGHT.background);
    }

    #[test]
    fn resolve_prefers_runtime_overrides() {
        let mut overrides = Colors::default();
        overrides[1] = Some(alacritty_terminal::vte::ansi::Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(
            DARK.resolve(Color::Indexed(1), &overrides),
            Color32::from_rgb(1, 2, 3)
        );
        // Unset slots fall back to the defaults.
        assert_eq!(DARK.resolve(Color::Indexed(2), &overrides), BASE[2]);
    }

    #[test]
    fn resolve_spec_is_verbatim() {
        let overrides = Colors::default();
        let rgb = alacritty_terminal::vte::ansi::Rgb {
            r: 10,
            g: 20,
            b: 30,
        };
        assert_eq!(
            DARK.resolve(Color::Spec(rgb), &overrides),
            Color32::from_rgb(10, 20, 30)
        );
    }

    #[test]
    fn for_theme_picks_the_matching_palette() {
        assert_eq!(Palette::for_theme(true), DARK);
        assert_eq!(Palette::for_theme(false), LIGHT);
    }

    #[test]
    fn the_ansi_slots_are_not_themed() {
        // Programs mean specific colours by these. The contrast floor, not a second table,
        // is what keeps them visible on both backgrounds.
        let overrides = Colors::default();
        for index in 0..=255u8 {
            assert_eq!(
                DARK.resolve(Color::Indexed(index), &overrides),
                LIGHT.resolve(Color::Indexed(index), &overrides),
                "index {index} must not depend on the theme"
            );
        }
    }

    #[test]
    fn the_defaults_are_legible_on_their_own_background() {
        for palette in [DARK, LIGHT] {
            for (what, colour) in [
                ("foreground", palette.foreground),
                ("cursor", palette.cursor),
            ] {
                let ratio = contrast(colour, palette.background);
                assert!(
                    ratio >= 4.5,
                    "{what} has contrast {ratio:.2} (dark_mode={})",
                    palette == DARK
                );
            }
        }
    }

    /// Selected text keeps its own colour, so the selection wash has to leave it readable.
    #[test]
    fn selection_keeps_the_foreground_readable() {
        for palette in [DARK, LIGHT] {
            assert!(
                contrast(palette.foreground, palette.selection) >= 3.0,
                "selected text is unreadable (dark_mode={})",
                palette == DARK
            );
        }
    }

    /// The whole point of the second palette: every ANSI slot has to end up visible on the
    /// background it is drawn on, in both themes. Without `readable` this fails for black on
    /// the dark background and for six pale slots on the light one.
    #[test]
    fn every_ansi_slot_is_visible_after_the_contrast_floor() {
        for palette in [DARK, LIGHT] {
            for (slot, colour) in BASE.iter().enumerate() {
                let shown = palette.readable(*colour, palette.background);
                let ratio = contrast(shown, palette.background);
                assert!(
                    ratio >= MIN_CONTRAST - 0.01,
                    "slot {slot} still has contrast {ratio:.2} (dark_mode={})",
                    palette == DARK
                );
            }
        }
    }

    /// ...and the floor has to leave the great majority of colours completely alone, or it is
    /// just a filter over the user's colour scheme.
    #[test]
    fn the_contrast_floor_only_touches_what_it_must() {
        let adjusted = |palette: Palette| {
            BASE.iter()
                .filter(|c| palette.readable(**c, palette.background) != **c)
                .count()
        };
        // Dark: ANSI black, and nothing else. Blue and magenta are the closest calls at
        // 2.24 and 2.09, and both are left alone.
        assert_eq!(adjusted(DARK), 1, "the dark theme should barely be touched");
        assert_ne!(DARK.readable(BASE[0], DARK.background), BASE[0]);
        assert_eq!(DARK.readable(BASE[12], DARK.background), BASE[12], "blue");
        assert_eq!(DARK.readable(BASE[5], DARK.background), BASE[5], "magenta");
        // Light: the six pale slots white would swallow - yellow, white, and bright
        // green/yellow/cyan/white.
        assert_eq!(adjusted(LIGHT), 6);
        for slot in [3, 7, 10, 11, 14, 15] {
            assert_ne!(
                LIGHT.readable(BASE[slot], LIGHT.background),
                BASE[slot],
                "slot {slot} is unreadable on white and must be nudged"
            );
        }
        // Anything already legible is returned byte-for-byte.
        assert_eq!(DARK.readable(BASE[1], DARK.background), BASE[1]);
        assert_eq!(LIGHT.readable(BASE[1], LIGHT.background), BASE[1]);
    }

    /// A nudge must not throw the hue away — red stays red rather than becoming white.
    #[test]
    fn the_contrast_floor_preserves_hue() {
        // Dark red on a dark background is the awkward case: it has to brighten, not wash out.
        let dark_red = Color32::from_rgb(60, 0, 0);
        let fixed = DARK.readable(dark_red, DARK.background);
        assert!(contrast(fixed, DARK.background) >= MIN_CONTRAST - 0.01);
        assert!(
            fixed.r() > fixed.g() && fixed.r() > fixed.b(),
            "{fixed:?} is no longer red"
        );
    }

    /// Hidden text relies on foreground == background, so nothing may rescue it. The check
    /// lives in the renderer; this pins the property the renderer depends on.
    #[test]
    fn the_contrast_floor_would_otherwise_reveal_hidden_text() {
        let bg = DARK.background;
        assert_ne!(
            DARK.readable(bg, bg),
            bg,
            "readable() does raise identical colours apart, which is why the renderer has to \
             skip it for HIDDEN cells"
        );
    }

    /// Dim has to mean "less visible" in both themes. Scaling brightness, which is what this
    /// used to do, makes dim text on a light background *more* prominent than normal text.
    #[test]
    fn dim_always_reduces_contrast() {
        for palette in [DARK, LIGHT] {
            let normal = contrast(palette.foreground, palette.background);
            let dimmed = contrast(palette.dim(palette.foreground), palette.background);
            assert!(
                dimmed < normal,
                "dim raised contrast from {normal:.2} to {dimmed:.2} (dark_mode={})",
                palette == DARK
            );
        }
    }
}
