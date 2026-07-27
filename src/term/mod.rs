//! Terminal buffer, emulator and views.
//!
//! Scrollback has one owner: [`TermBuffer`], held by the tab rather than by the connection.
//! The Tauri build split it between Rust (`Port::output`) and the browser
//! (`Connection.rx_buffer`), and cleared it on every connect, so scrollback died with the
//! transport. Plan task 3 needs the opposite — a reconnect that preserves the terminal
//! window — so the buffer outlives any one transport.
//!
//! # Why the raw byte ring still exists
//!
//! ANSI mode renders from [`emu::Emulator`], a real terminal screen. An emulator is
//! *stateful*: it must see every byte in order, and cannot be handed the tail of a stream and
//! produce a correct screen. So the raw bytes stay the source of truth and the emulator is a
//! derived view fed forward from them. That is what makes switching display modes, or
//! changing the scrollback limit, tractable — both just rebuild the emulator by replaying.
//!
//! The byte-oriented views (ASCII, decimal, hex) read the ring directly.

pub mod emu;
pub mod input;
pub mod palette;
pub mod render;
pub mod text;

/// Default scrollback, in bytes.
pub const DEFAULT_MAX_BYTES: usize = 200_000;
/// Bounds of the scrollback slider.
pub const MIN_MAX_BYTES: usize = 2_000;
pub const MAX_MAX_BYTES: usize = 2_000_000;

/// Received bytes plus a line index over them.
///
/// The line index is maintained incrementally on append so drawing a screenful of the
/// byte-oriented views never costs more than a screenful of work.
pub struct TermBuffer {
    /// Retained bytes.
    bytes: Vec<u8>,
    /// How many bytes have been dropped off the front over this buffer's lifetime.
    discarded: u64,
    /// Retention limit.
    max_bytes: usize,
    /// Absolute offset of each logical line's first byte. Always non-empty.
    lines: Vec<u64>,
    /// Bumped on every mutation so views can invalidate caches.
    revision: u64,
    /// Total bytes ever received.
    total_received: u64,
}

impl Default for TermBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_BYTES)
    }
}

impl TermBuffer {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::new(),
            discarded: 0,
            max_bytes: max_bytes.clamp(MIN_MAX_BYTES, MAX_MAX_BYTES),
            lines: vec![0],
            revision: 0,
            total_received: 0,
        }
    }

    pub fn total_received(&self) -> u64 {
        self.total_received
    }

    pub fn retained_bytes(&self) -> usize {
        self.bytes.len()
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Number of logical lines currently held.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Change the retention limit, trimming immediately if it shrank.
    ///
    /// The old implementation padded the buffer with NUL bytes when the limit *grew*, to
    /// keep the UI's byte accounting in step. Nothing needs that now, so growing simply
    /// raises the ceiling.
    pub fn set_max_bytes(&mut self, max_bytes: usize) {
        let max_bytes = max_bytes.clamp(MIN_MAX_BYTES, MAX_MAX_BYTES);
        if max_bytes == self.max_bytes {
            return;
        }
        self.max_bytes = max_bytes;
        self.trim();
        self.revision += 1;
    }

    /// Append newly received bytes.
    pub fn append(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.total_received += data.len() as u64;

        let scan_from = self.bytes.len();
        self.bytes.extend_from_slice(data);

        for (offset, byte) in data.iter().enumerate() {
            if *byte == b'\n' {
                self.lines
                    .push(self.discarded + (scan_from + offset + 1) as u64);
            }
        }

        self.trim();
        self.revision += 1;
    }

    /// Drop everything. Backs the Clear button.
    pub fn clear(&mut self) {
        self.discarded += self.bytes.len() as u64;
        self.bytes.clear();
        self.lines = vec![self.discarded];
        self.revision += 1;
    }

    /// Absolute offset -> index into `bytes`.
    fn rel(&self, absolute: u64) -> usize {
        (absolute - self.discarded) as usize
    }

    /// Enforce the retention limit.
    ///
    /// Trimming is done in chunks rather than on every byte: `Vec::drain` from the front is
    /// O(len), so trimming to the exact limit on each append would make a busy port O(n)
    /// per byte. Allowing 25% overshoot makes it amortised O(1).
    fn trim(&mut self) {
        let slack = self.max_bytes / 4;
        if self.bytes.len() <= self.max_bytes + slack {
            return;
        }

        let excess = self.bytes.len() - self.max_bytes;
        self.bytes.drain(..excess);
        self.discarded += excess as u64;

        // Drop line entries that fell entirely off the front; the line straddling the new
        // start survives with its offset clamped.
        let keep_from = self
            .lines
            .partition_point(|start| *start <= self.discarded)
            .saturating_sub(1);
        self.lines.drain(..keep_from);
        if let Some(first) = self.lines.first_mut() {
            *first = (*first).max(self.discarded);
        }
        if self.lines.is_empty() {
            self.lines.push(self.discarded);
        }
    }

    /// Bytes of logical line `index`, without its trailing newline.
    pub fn line(&self, index: usize) -> Option<&[u8]> {
        let start = self.rel(*self.lines.get(index)?);
        let end = match self.lines.get(index + 1) {
            Some(next) => self.rel(*next),
            None => self.bytes.len(),
        };
        let slice = self.bytes.get(start..end)?;
        // Strip the line terminator so renderers do not have to.
        let slice = slice.strip_suffix(b"\n").unwrap_or(slice);
        Some(slice.strip_suffix(b"\r").unwrap_or(slice))
    }

    /// All retained bytes, for the byte-oriented display modes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Retained bytes from absolute offset `from` onwards.
    ///
    /// Returns the slice and the absolute offset it actually starts at, which is later than
    /// requested when the ring has already trimmed past it. Feeding the emulator uses this,
    /// so a UI stall during a flood degrades to a reported gap rather than silent corruption.
    pub fn slice_from(&self, from: u64) -> (&[u8], u64) {
        if from >= self.total_received {
            return (&[], self.total_received);
        }
        let start = from.max(self.discarded);
        (&self.bytes[self.rel(start)..], start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(buf: &TermBuffer, i: usize) -> String {
        String::from_utf8_lossy(buf.line(i).unwrap()).into_owned()
    }

    #[test]
    fn empty_buffer_has_one_empty_line() {
        let buf = TermBuffer::new(1000);
        assert_eq!(buf.line_count(), 1);
        assert_eq!(line_text(&buf, 0), "");
    }

    #[test]
    fn lines_are_split_on_newline() {
        let mut buf = TermBuffer::new(10_000);
        buf.append(b"one\ntwo\nthree");
        assert_eq!(buf.line_count(), 3);
        assert_eq!(line_text(&buf, 0), "one");
        assert_eq!(line_text(&buf, 1), "two");
        assert_eq!(line_text(&buf, 2), "three");
    }

    #[test]
    fn crlf_is_stripped_from_line_text() {
        let mut buf = TermBuffer::new(10_000);
        buf.append(b"one\r\ntwo\r\n");
        assert_eq!(line_text(&buf, 0), "one");
        assert_eq!(line_text(&buf, 1), "two");
    }

    #[test]
    fn appending_byte_at_a_time_matches_one_shot() {
        let mut a = TermBuffer::new(10_000);
        a.append(b"one\ntwo\nthree");

        let mut b = TermBuffer::new(10_000);
        for byte in b"one\ntwo\nthree" {
            b.append(&[*byte]);
        }

        assert_eq!(a.line_count(), b.line_count());
        for i in 0..a.line_count() {
            assert_eq!(line_text(&a, i), line_text(&b, i));
        }
    }

    #[test]
    fn trim_enforces_the_limit_and_keeps_reading_correct() {
        let mut buf = TermBuffer::new(MIN_MAX_BYTES);
        for i in 0..2000 {
            buf.append(format!("line {i}\n").as_bytes());
        }
        assert!(buf.retained_bytes() <= MIN_MAX_BYTES + MIN_MAX_BYTES / 4);
        let last = line_text(&buf, buf.line_count() - 2);
        assert_eq!(last, "line 1999");
    }

    #[test]
    fn every_line_is_readable_after_trimming() {
        let mut buf = TermBuffer::new(MIN_MAX_BYTES);
        for i in 0..5000 {
            buf.append(format!("{i}\n").as_bytes());
        }
        for i in 0..buf.line_count() {
            assert!(buf.line(i).is_some(), "line {i} should be readable");
        }
    }

    #[test]
    fn shrinking_max_bytes_trims_immediately() {
        let mut buf = TermBuffer::new(MAX_MAX_BYTES);
        buf.append(&[b'x'; 100_000]);
        assert_eq!(buf.retained_bytes(), 100_000);
        buf.set_max_bytes(MIN_MAX_BYTES);
        assert!(buf.retained_bytes() <= MIN_MAX_BYTES + MIN_MAX_BYTES / 4);
    }

    #[test]
    fn growing_max_bytes_does_not_pad() {
        // The Tauri build inserted NUL padding when the limit grew.
        let mut buf = TermBuffer::new(MIN_MAX_BYTES);
        buf.append(b"hello\n");
        buf.set_max_bytes(MAX_MAX_BYTES);
        assert_eq!(buf.retained_bytes(), 6);
        assert_eq!(line_text(&buf, 0), "hello");
    }

    #[test]
    fn clear_resets_content_but_not_totals() {
        let mut buf = TermBuffer::new(10_000);
        buf.append(b"hello\nworld\n");
        buf.clear();
        assert_eq!(buf.line_count(), 1);
        assert_eq!(buf.retained_bytes(), 0);
        assert_eq!(buf.total_received(), 12);
        buf.append(b"again\n");
        assert_eq!(line_text(&buf, 0), "again");
    }

    #[test]
    fn revision_changes_on_mutation() {
        let mut buf = TermBuffer::new(10_000);
        let r0 = buf.revision;
        buf.append(b"x");
        assert_ne!(r0, buf.revision);
        let r1 = buf.revision;
        buf.clear();
        assert_ne!(r1, buf.revision);
    }

    #[test]
    fn empty_append_is_a_no_op() {
        let mut buf = TermBuffer::new(10_000);
        let r = buf.revision;
        buf.append(b"");
        assert_eq!(r, buf.revision);
    }

    #[test]
    fn max_bytes_is_clamped_to_bounds() {
        assert_eq!(TermBuffer::new(1).max_bytes(), MIN_MAX_BYTES);
        assert_eq!(TermBuffer::new(usize::MAX).max_bytes(), MAX_MAX_BYTES);
    }

    #[test]
    fn slice_from_returns_the_unfed_tail() {
        let mut buf = TermBuffer::new(10_000);
        buf.append(b"hello");
        let (slice, at) = buf.slice_from(0);
        assert_eq!(slice, b"hello");
        assert_eq!(at, 0);

        let (slice, at) = buf.slice_from(3);
        assert_eq!(slice, b"lo");
        assert_eq!(at, 3);
    }

    #[test]
    fn slice_from_the_end_is_empty() {
        let mut buf = TermBuffer::new(10_000);
        buf.append(b"hello");
        let (slice, at) = buf.slice_from(5);
        assert!(slice.is_empty());
        assert_eq!(at, 5);
        // Past the end is clamped, not a panic.
        let (slice, at) = buf.slice_from(99);
        assert!(slice.is_empty());
        assert_eq!(at, 5);
    }

    #[test]
    fn slice_from_reports_a_gap_after_trimming() {
        let mut buf = TermBuffer::new(MIN_MAX_BYTES);
        for _ in 0..2000 {
            buf.append(&[b'x'; 100]);
        }
        // Offset 0 is long gone; the reported start is where data actually resumes.
        let (slice, at) = buf.slice_from(0);
        assert!(at > 0, "must report that early bytes were dropped");
        assert_eq!(slice.len(), buf.retained_bytes());
        assert_eq!(at + slice.len() as u64, buf.total_received());
    }

    #[test]
    fn slice_from_is_contiguous_across_appends() {
        // Feeding forward from `slice_from` must reconstruct the stream exactly.
        let mut buf = TermBuffer::new(MAX_MAX_BYTES);
        let mut fed = 0u64;
        let mut seen = Vec::new();
        for i in 0..50 {
            buf.append(format!("chunk{i};").as_bytes());
            let (slice, at) = buf.slice_from(fed);
            assert_eq!(at, fed, "no gap expected with a large ring");
            seen.extend_from_slice(slice);
            fed += slice.len() as u64;
        }
        assert_eq!(seen, buf.bytes());
        assert_eq!(fed, buf.total_received());
    }
}
