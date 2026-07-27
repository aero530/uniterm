//! Terminal views.
//!
//! Two shapes of view, because ANSI mode and the byte-oriented modes are genuinely different
//! things:
//!
//! * [`grid_view`] draws [`Emulator`]'s screen — a fixed viewport of `cols x rows` cells. The
//!   emulator owns the scrollback and the scroll position, exactly as a real terminal does,
//!   so there is no egui scroll area; the wheel drives `display_offset`.
//! * [`buffer_view`] draws the raw byte ring for the ASCII, decimal and hex modes, in an egui
//!   scroll area virtualized with [`egui::ScrollArea::show_rows`].
//!
//! Virtualization is load-bearing in both: egui builds a text galley per label per frame, so
//! laying out a full scrollback every frame would stall the UI. The grid only ever touches
//! visible cells, and `show_rows` only visible rows.

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape;
use eframe::egui::{
    self, Color32, FontId, Id, Label, Pos2, Rect, Sense, Stroke, TextFormat, Ui, Vec2,
    text::LayoutJob,
};

use super::emu::Emulator;
use super::palette;
use super::text;
use super::TermBuffer;
use crate::settings::DisplayMode;

/// Border drawn while a view holds keyboard focus, so it is obvious that typing is being
/// transmitted. The Svelte view used a red border for the same reason.
const FOCUS_BORDER: Color32 = Color32::from_rgb(220, 60, 50);
/// Width of the scrollback indicator down the right edge of the grid.
const SCROLLBAR_WIDTH: f32 = 8.0;
/// Upper bound on bytes per row, so a pathological width cannot allocate wildly.
const MAX_BYTES_PER_ROW: usize = 4096;

/// What a view reported back to the caller.
pub struct TerminalResponse {
    /// The view holds keyboard focus, so key events should be transmitted.
    pub focused: bool,
}

/// Monospace cell metrics for a font size.
fn metrics(ui: &Ui, font: &FontId) -> (f32, f32) {
    ui.ctx()
        .fonts_mut(|f| (f.row_height(font), f.glyph_width(font, 'M')))
}

/// Keep Tab, the arrows and Escape from moving focus while the view has it — the remote end
/// expects to receive all of them.
fn lock_focus(ui: &mut Ui, id: Id) {
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            id,
            egui::EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
            },
        );
    });
}

fn view_frame(ui: &Ui, focused: bool) -> egui::Frame {
    let border = if focused {
        Stroke::new(2.0, FOCUS_BORDER)
    } else {
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
    };
    egui::Frame::default()
        .fill(palette::BACKGROUND)
        .stroke(border)
        .inner_margin(4.0)
}

// ---------------------------------------------------------------------------------------
// ANSI: the emulator's screen
// ---------------------------------------------------------------------------------------

/// Where the grid sits on screen and how big its cells are.
#[derive(Clone, Copy, Debug)]
struct Geometry {
    origin: Pos2,
    char_width: f32,
    row_height: f32,
    columns: usize,
    rows: usize,
}

impl Geometry {
    /// Rect of a single cell.
    fn cell(&self, line: i32, column: usize) -> Rect {
        Rect::from_min_size(
            Pos2::new(
                self.origin.x + column as f32 * self.char_width,
                self.origin.y + line as f32 * self.row_height,
            ),
            Vec2::new(self.char_width, self.row_height),
        )
    }

    /// Rect spanning `columns` cells starting at `column`.
    fn span(&self, line: i32, column: usize, columns: usize) -> Rect {
        let cell = self.cell(line, column);
        Rect::from_min_size(
            cell.min,
            Vec2::new(columns as f32 * self.char_width, self.row_height),
        )
    }

    fn size(&self) -> Vec2 {
        Vec2::new(
            self.columns as f32 * self.char_width,
            self.rows as f32 * self.row_height,
        )
    }

    /// Convert a screen position into a grid point, undoing the scrollback offset.
    fn point_at(&self, pos: Pos2, display_offset: usize) -> (Point, Side) {
        let fx = ((pos.x - self.origin.x) / self.char_width).max(0.0);
        let fy = ((pos.y - self.origin.y) / self.row_height).max(0.0);
        let column = (fx.floor() as usize).min(self.columns.saturating_sub(1));
        let row = (fy.floor() as usize).min(self.rows.saturating_sub(1));
        let line = Line(row as i32 - display_offset as i32);
        let side = if fx.fract() > 0.5 { Side::Right } else { Side::Left };
        (Point::new(line, Column(column)), side)
    }
}

/// One stretch of same-styled, single-width cells on one row.
struct Run {
    line: i32,
    start_column: usize,
    columns: usize,
    text: String,
    fg: Color32,
    bg: Color32,
    flags: Flags,
}

impl Run {
    fn reset(&mut self, line: i32, column: usize, fg: Color32, bg: Color32, flags: Flags) {
        self.line = line;
        self.start_column = column;
        self.columns = 0;
        self.text.clear();
        self.fg = fg;
        self.bg = bg;
        self.flags = flags;
    }
}

/// Draw the emulator's screen.
pub fn grid_view(
    ui: &mut Ui,
    id: Id,
    emu: &mut Emulator,
    font_size: f32,
    height: f32,
) -> TerminalResponse {
    let font = FontId::monospace(font_size);
    let (row_height, char_width) = metrics(ui, &font);
    let has_focus = ui.memory(|m| m.has_focus(id));
    if has_focus {
        lock_focus(ui, id);
    }

    let frame = view_frame(ui, has_focus);
    let viewport_height = (height - frame.total_margin().sum().y).max(row_height);

    frame.show(ui, |ui| {
        let available = Vec2::new(ui.available_width(), viewport_height);
        // Leave room for the scrollback indicator so cells never hide beneath it.
        let grid_width = (available.x - SCROLLBAR_WIDTH).max(char_width);

        let columns = if char_width > 0.0 {
            (grid_width / char_width).floor().max(1.0) as usize
        } else {
            1
        };
        let rows = if row_height > 0.0 {
            (viewport_height / row_height).floor().max(1.0) as usize
        } else {
            1
        };
        emu.resize(columns, rows);

        let (rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());
        let geometry = Geometry {
            origin: rect.min,
            char_width,
            row_height,
            columns,
            rows,
        };

        handle_scroll(ui, emu, &response, row_height);
        handle_selection(ui, emu, &response, geometry);
        paint_grid(ui, emu, geometry, &font, has_focus);
        scrollback_indicator(ui, emu, rect, rows);

        if response.clicked() {
            response.request_focus();
        }
    });

    TerminalResponse {
        focused: ui.memory(|m| m.has_focus(id)),
    }
}

/// Mouse wheel and page keys drive the emulator's scroll position.
fn handle_scroll(ui: &Ui, emu: &mut Emulator, response: &egui::Response, row_height: f32) {
    if response.hovered() && row_height > 0.0 {
        let delta = ui.input(|i| i.smooth_scroll_delta.y);
        if delta != 0.0 {
            let lines = (delta / row_height).round() as i32;
            if lines != 0 {
                emu.scroll_lines(lines);
            }
        }
    }
    if response.has_focus() {
        // Shift+PageUp/Down scrolls the view; bare PageUp/Down is data for the remote end.
        let (page_up, page_down) = ui.input(|i| {
            (
                i.modifiers.shift && i.key_pressed(egui::Key::PageUp),
                i.modifiers.shift && i.key_pressed(egui::Key::PageDown),
            )
        });
        if page_up {
            emu.page(true);
        }
        if page_down {
            emu.page(false);
        }
    }
}

fn handle_selection(ui: &Ui, emu: &mut Emulator, response: &egui::Response, geometry: Geometry) {
    if geometry.char_width <= 0.0 || geometry.row_height <= 0.0 {
        return;
    }
    let offset = emu.display_offset();
    // Alt turns a drag into a rectangular selection, matching most terminals.
    let block = ui.input(|i| i.modifiers.alt);

    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            let (point, side) = geometry.point_at(pos, offset);
            emu.start_selection(point, side, block);
        }
    } else if response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            let (point, side) = geometry.point_at(pos, offset);
            emu.update_selection(point, side);
        }
    } else if response.clicked() {
        emu.clear_selection();
    }
}

/// Paint the visible cells.
///
/// `display_iter` yields cells in row-major order, so runs of same-styled cells can be
/// emitted as they are walked, without collecting the grid into an intermediate buffer.
fn paint_grid(ui: &Ui, emu: &Emulator, geometry: Geometry, font: &FontId, focused: bool) {
    let grid_rect = Rect::from_min_size(geometry.origin, geometry.size());
    let painter = ui.painter().with_clip_rect(grid_rect);
    let content = emu.term().renderable_content();
    let offset = content.display_offset as i32;
    let colors = content.colors;
    let selection = content.selection;
    let cursor_point = content.cursor.point;
    let cursor_shape = content.cursor.shape;
    let show_cursor = content.mode.contains(TermMode::SHOW_CURSOR);

    let mut run = Run {
        line: i32::MIN,
        start_column: 0,
        columns: 0,
        text: String::new(),
        fg: palette::FOREGROUND,
        bg: palette::BACKGROUND,
        flags: Flags::empty(),
    };

    for indexed in content.display_iter {
        let cell = indexed.cell;
        let flags = cell.flags;

        // The trailing half of a wide character carries no glyph of its own.
        if flags.contains(Flags::WIDE_CHAR_SPACER) || flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }

        let line = indexed.point.line.0 + offset;
        let column = indexed.point.column.0;
        let wide = flags.contains(Flags::WIDE_CHAR);

        let mut fg = palette::resolve_foreground(cell.fg, colors, flags.contains(Flags::BOLD));
        let mut bg = palette::resolve(cell.bg, colors);
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if selection.is_some_and(|range| range.contains(indexed.point)) {
            bg = palette::SELECTION;
        }
        if flags.contains(Flags::HIDDEN) {
            fg = bg;
        } else if flags.contains(Flags::DIM) {
            fg = fg.gamma_multiply(0.66);
        }

        // A run holds cells that share style, sit on one line and are all single width.
        let continues = run.line == line
            && run.start_column + run.columns == column
            && run.fg == fg
            && run.bg == bg
            && style_flags(run.flags) == style_flags(flags)
            && !wide
            && run.columns > 0;

        if !continues {
            flush_run(&painter, &run, geometry, font);
            run.reset(line, column, fg, bg, flags);
        }

        run.text.push(cell.c);
        run.columns += if wide { 2 } else { 1 };

        // A wide glyph is always its own run so column arithmetic stays exact.
        if wide {
            flush_run(&painter, &run, geometry, font);
            run.columns = 0;
            run.text.clear();
        }
    }
    flush_run(&painter, &run, geometry, font);

    // Cursor last, so it sits above the cell it occupies.
    if show_cursor && cursor_shape != CursorShape::Hidden {
        let line = cursor_point.line.0 + offset;
        if line >= 0 && (line as usize) < geometry.rows {
            paint_cursor(
                &painter,
                geometry,
                line,
                cursor_point.column.0,
                cursor_shape,
                focused,
            );
        }
    }
}

/// Only the flags that change how text is drawn.
fn style_flags(flags: Flags) -> Flags {
    flags
        & (Flags::BOLD
            | Flags::ITALIC
            | Flags::INVERSE
            | Flags::DIM
            | Flags::HIDDEN
            | Flags::STRIKEOUT
            | Flags::ALL_UNDERLINES)
}

fn flush_run(painter: &egui::Painter, run: &Run, geometry: Geometry, font: &FontId) {
    if run.columns == 0 || run.text.is_empty() {
        return;
    }
    let cell_rect = geometry.span(run.line, run.start_column, run.columns);

    if run.bg != palette::BACKGROUND {
        painter.rect_filled(cell_rect, 0.0, run.bg);
    }

    // Blank runs need no glyphs drawn, but their background still matters.
    if run.text.trim().is_empty() {
        return;
    }

    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    job.append(
        &run.text,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color: run.fg,
            italics: run.flags.contains(Flags::ITALIC),
            underline: if run.flags.intersects(Flags::ALL_UNDERLINES) {
                Stroke::new(1.0, run.fg)
            } else {
                Stroke::NONE
            },
            strikethrough: if run.flags.contains(Flags::STRIKEOUT) {
                Stroke::new(1.0, run.fg)
            } else {
                Stroke::NONE
            },
            ..Default::default()
        },
    );
    let galley = painter.layout_job(job);
    painter.galley(cell_rect.left_top(), galley, run.fg);
}

fn paint_cursor(
    painter: &egui::Painter,
    geometry: Geometry,
    line: i32,
    column: usize,
    shape: CursorShape,
    focused: bool,
) {
    let cell = geometry.cell(line, column);
    let (char_width, row_height) = (geometry.char_width, geometry.row_height);

    // An unfocused terminal shows a hollow cursor, so it is clear typing goes elsewhere.
    if !focused {
        painter.rect_stroke(
            cell,
            0.0,
            Stroke::new(1.0, palette::CURSOR),
            egui::StrokeKind::Inside,
        );
        return;
    }

    match shape {
        CursorShape::Block => {
            painter.rect_filled(cell, 0.0, palette::CURSOR);
        }
        CursorShape::HollowBlock => {
            painter.rect_stroke(
                cell,
                0.0,
                Stroke::new(1.0, palette::CURSOR),
                egui::StrokeKind::Inside,
            );
        }
        CursorShape::Beam => {
            painter.rect_filled(
                Rect::from_min_size(cell.left_top(), Vec2::new(2.0, row_height)),
                0.0,
                palette::CURSOR,
            );
        }
        CursorShape::Underline => {
            painter.rect_filled(
                Rect::from_min_size(
                    Pos2::new(cell.left(), cell.bottom() - 2.0),
                    Vec2::new(char_width, 2.0),
                ),
                0.0,
                palette::CURSOR,
            );
        }
        CursorShape::Hidden => {}
    }
}

/// A draggable indicator showing where the viewport sits in the scrollback.
fn scrollback_indicator(ui: &mut Ui, emu: &mut Emulator, rect: Rect, rows: usize) {
    let history = emu.history_size();
    if history == 0 {
        return;
    }
    let total = history + rows;
    let track = Rect::from_min_size(
        Pos2::new(rect.right() - SCROLLBAR_WIDTH, rect.top()),
        Vec2::new(SCROLLBAR_WIDTH, rect.height()),
    );

    // Registered after the grid, so it wins the pointer over this strip.
    let response = ui.interact(track, ui.id().with("scrollback"), Sense::click_and_drag());

    let visible_fraction = (rows as f32 / total as f32).clamp(0.05, 1.0);
    let thumb_height = (track.height() * visible_fraction).max(16.0);
    // display_offset counts lines above the bottom; 0 means "at the newest output".
    let scrolled = emu.display_offset() as f32 / history as f32;
    let thumb_y = track.top() + (track.height() - thumb_height) * (1.0 - scrolled);

    if response.dragged() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let usable = (track.height() - thumb_height).max(1.0);
            let fraction = ((pos.y - track.top() - thumb_height / 2.0) / usable).clamp(0.0, 1.0);
            emu.scroll_to(((1.0 - fraction) * history as f32).round() as usize);
        }
    }

    let painter = ui.painter();
    painter.rect_filled(track, 2.0, ui.visuals().extreme_bg_color.gamma_multiply(0.5));
    let thumb = Rect::from_min_size(
        Pos2::new(track.left() + 1.0, thumb_y),
        Vec2::new(track.width() - 2.0, thumb_height),
    );
    let colour = if response.hovered() || response.dragged() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().widgets.inactive.bg_fill
    };
    painter.rect_filled(thumb, 2.0, colour);
}

// ---------------------------------------------------------------------------------------
// ASCII / decimal / hex: byte views over the ring
// ---------------------------------------------------------------------------------------

/// Draw the raw byte ring.
pub fn buffer_view(
    ui: &mut Ui,
    id: Id,
    buffer: &TermBuffer,
    mode: DisplayMode,
    font_size: f32,
    height: f32,
) -> TerminalResponse {
    let font = FontId::monospace(font_size);
    let (row_height, char_width) = metrics(ui, &font);
    let has_focus = ui.memory(|m| m.has_focus(id));
    if has_focus {
        lock_focus(ui, id);
    }

    let frame = view_frame(ui, has_focus);
    let viewport_height = (height - frame.total_margin().sum().y).max(row_height);

    let inner = frame.show(ui, |ui| {
        ui.style_mut().visuals.override_text_color = Some(palette::FOREGROUND);
        ui.spacing_mut().item_spacing.y = 0.0;
        let available_width = ui.available_width();

        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .max_height(viewport_height)
            .min_scrolled_height(viewport_height)
            .id_salt(id.with("scroll"))
            .show_rows(
                ui,
                row_height,
                row_count(buffer, mode, available_width, char_width),
                |ui, rows| {
                    for row in rows {
                        let job = match mode {
                            DisplayMode::Ascii => text_row(buffer, row, &font),
                            _ => byte_row(
                                buffer,
                                row,
                                mode,
                                &font,
                                bytes_per_row(mode, available_width, char_width),
                            ),
                        };
                        ui.add(
                            Label::new(job)
                                .selectable(true)
                                .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    }
                },
            );
    });

    let click = ui.interact(inner.response.rect, id, Sense::click());
    if click.clicked() {
        click.request_focus();
    }

    TerminalResponse {
        focused: ui.memory(|m| m.has_focus(id)),
    }
}

/// How many rows the byte view has in this mode.
fn row_count(buffer: &TermBuffer, mode: DisplayMode, width: f32, char_width: f32) -> usize {
    match mode {
        DisplayMode::Ascii => buffer.line_count(),
        _ => {
            let per_row = bytes_per_row(mode, width, char_width);
            buffer.bytes().len().div_ceil(per_row).max(1)
        }
    }
}

/// Bytes shown per row in the numeric modes, chosen to fill the available width.
///
/// Guards against a zero or non-finite cell width: a float-to-int cast saturates in Rust, so
/// an infinite quotient would yield `usize::MAX` and overflow the slice arithmetic in
/// [`byte_row`]. That is reachable in practice — `char_width` is 0 before the font atlas has
/// been built.
fn bytes_per_row(mode: DisplayMode, width: f32, char_width: f32) -> usize {
    // "255 " is 4 columns, "0xff " is 5.
    let token_columns = if matches!(mode, DisplayMode::Decimal) { 4.0 } else { 5.0 };
    let cell = char_width * token_columns;
    if !cell.is_finite() || cell <= 0.0 || !width.is_finite() {
        return 1;
    }
    ((width / cell).floor() as usize).clamp(1, MAX_BYTES_PER_ROW)
}

fn plain_format(font: &FontId) -> TextFormat {
    TextFormat {
        font_id: font.clone(),
        color: palette::FOREGROUND,
        ..Default::default()
    }
}

fn new_job() -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    job
}

fn text_row(buffer: &TermBuffer, row: usize, font: &FontId) -> LayoutJob {
    let mut job = new_job();
    if let Some(bytes) = buffer.line(row) {
        job.append(&text::visible(bytes), 0.0, plain_format(font));
    }
    job
}

fn byte_row(
    buffer: &TermBuffer,
    row: usize,
    mode: DisplayMode,
    font: &FontId,
    per_row: usize,
) -> LayoutJob {
    let mut job = new_job();
    let bytes = buffer.bytes();
    let start = row * per_row;
    if start >= bytes.len() {
        return job;
    }
    let end = start.saturating_add(per_row).min(bytes.len());

    let mut out = String::with_capacity(per_row * 5);
    for byte in &bytes[start..end] {
        match mode {
            DisplayMode::Decimal => out.push_str(&format!("{byte:<3} ")),
            _ => out.push_str(&format!("{byte:#04x} ")),
        }
    }
    job.append(&out, 0.0, plain_format(font));
    job
}

/// The whole byte ring as plain text, for the clipboard.
pub fn plain_text(buffer: &TermBuffer, mode: DisplayMode) -> String {
    match mode {
        DisplayMode::Ansi | DisplayMode::Ascii => {
            let mut out = String::new();
            for row in 0..buffer.line_count() {
                if let Some(bytes) = buffer.line(row) {
                    out.push_str(&text::visible(bytes));
                }
                out.push('\n');
            }
            out
        }
        DisplayMode::Decimal => buffer
            .bytes()
            .iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(" "),
        DisplayMode::Hex => buffer
            .bytes()
            .iter()
            .map(|b| format!("{b:#04x}"))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_per_row_scales_with_width_and_is_never_zero() {
        assert_eq!(bytes_per_row(DisplayMode::Hex, 800.0, 8.0), 20);
        assert_eq!(bytes_per_row(DisplayMode::Decimal, 800.0, 8.0), 25);
        assert_eq!(bytes_per_row(DisplayMode::Hex, 0.0, 8.0), 1);
        assert_eq!(bytes_per_row(DisplayMode::Hex, 800.0, 0.0), 1);
        assert_eq!(bytes_per_row(DisplayMode::Hex, f32::INFINITY, 8.0), 1);
    }

    #[test]
    fn ascii_row_count_is_the_line_count() {
        let mut buf = TermBuffer::new(10_000);
        buf.append(b"a\nb\nc");
        assert_eq!(row_count(&buf, DisplayMode::Ascii, 800.0, 8.0), 3);
    }

    #[test]
    fn byte_row_count_covers_every_byte() {
        let mut buf = TermBuffer::new(10_000);
        buf.append(&[0u8; 45]);
        assert_eq!(row_count(&buf, DisplayMode::Hex, 800.0, 8.0), 3);
    }

    #[test]
    fn empty_buffer_still_has_one_row_in_every_mode() {
        let buf = TermBuffer::new(10_000);
        for mode in DisplayMode::ALL {
            assert!(row_count(&buf, *mode, 800.0, 8.0) >= 1, "{mode:?}");
        }
    }

    #[test]
    fn plain_text_round_trips_ascii() {
        let mut buf = TermBuffer::new(10_000);
        buf.append(b"one\ntwo\n");
        assert_eq!(plain_text(&buf, DisplayMode::Ascii), "one\ntwo\n\n");
    }

    #[test]
    fn plain_text_hex_formats_every_byte() {
        let mut buf = TermBuffer::new(10_000);
        buf.append(&[0x00, 0xff]);
        assert_eq!(plain_text(&buf, DisplayMode::Hex), "0x00 0xff");
    }

    #[test]
    fn style_flags_ignores_layout_only_flags() {
        // WRAPLINE and the wide-char markers must not split a run.
        let a = Flags::BOLD | Flags::WRAPLINE;
        let b = Flags::BOLD;
        assert_eq!(style_flags(a), style_flags(b));
        // Real style differences still split.
        assert_ne!(style_flags(Flags::BOLD), style_flags(Flags::ITALIC));
    }

    fn geometry(origin: Pos2, columns: usize, rows: usize) -> Geometry {
        Geometry {
            origin,
            char_width: 8.0,
            row_height: 16.0,
            columns,
            rows,
        }
    }

    #[test]
    fn point_at_maps_positions_to_grid_cells() {
        let g = geometry(Pos2::new(10.0, 20.0), 80, 24);
        // Third column, second row.
        let (point, side) = g.point_at(
            Pos2::new(10.0 + 2.0 * 8.0 + 1.0, 20.0 + 1.0 * 16.0 + 1.0),
            0,
        );
        assert_eq!(point.column, Column(2));
        assert_eq!(point.line, Line(1));
        assert_eq!(side, Side::Left, "left half of the cell");
    }

    #[test]
    fn point_at_reports_the_right_half_of_a_cell() {
        let g = geometry(Pos2::ZERO, 80, 24);
        let (_, side) = g.point_at(Pos2::new(7.0, 1.0), 0);
        assert_eq!(side, Side::Right);
    }

    #[test]
    fn point_at_undoes_the_scrollback_offset() {
        let g = geometry(Pos2::ZERO, 80, 24);
        // Top row while scrolled back 5 lines is grid line -5.
        let (point, _) = g.point_at(Pos2::ZERO, 5);
        assert_eq!(point.line, Line(-5));
    }

    #[test]
    fn point_at_clamps_inside_the_grid() {
        let g = geometry(Pos2::ZERO, 10, 4);
        let (point, _) = g.point_at(Pos2::new(9999.0, 9999.0), 0);
        assert_eq!(point.column, Column(9));
        assert_eq!(point.line, Line(3));
        // Negative positions clamp to the origin rather than underflowing.
        let (point, _) = g.point_at(Pos2::new(-50.0, -50.0), 0);
        assert_eq!(point.column, Column(0));
        assert_eq!(point.line, Line(0));
    }
}
