# UniTerm - Serial port terminal

![logo](resources/icons/128x128.png)

A native serial terminal built with [egui](https://github.com/emilk/egui) and
[egui_dock](https://github.com/anhosh/egui_dock).

## Features

* Dockable, splittable views — drag a tab header to split; drag tabs between panes or out
  into their own window
* **Full terminal emulation** in ANSI mode, built on
  [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal) — the same core
  Alacritty ships. Cursor addressing, erase, scroll regions, the alternate screen buffer,
  wide characters, reflow on resize, and base / bright / 256-colour / 24-bit colour, plus
  bold, dim, italic, underline, strikethrough and reverse video.
* Display received data as
  * ANSI — a real terminal screen
  * ASCII text — the raw view, with control bytes shown as caret notation (`^[`, `^C`, `^?`)
  * Decimal values
  * Hex values
* Send data as ASCII text, decimal values or hex values, with optional CR / LF
* Type directly into the terminal to transmit in real time: arrow keys, function keys,
  control combinations (Ctrl+C sends `0x03`), application-cursor-key mode and bracketed
  paste
* Select with the mouse and copy (Ctrl+Shift+C); Alt-drag for a rectangular selection
* Scroll back through history with the wheel, Shift+PageUp/PageDown, or the indicator on the
  right edge
* The terminal resizes with the pane, and answers terminal queries (device attributes,
  cursor position) so programs that ask do not hang
* Log received data to file
* Manage multiple port connections
* Notices an unplugged adapter and reports why a connection dropped

## Install

Download the latest release from [releases](https://github.com/aero530/uniterm/releases).

## Roadmap

See [PLAN.md](PLAN.md) for the full plan and sizing. In short:

* SSH connections alongside serial
* A reconnect button that preserves the terminal contents
* Session and layout persistence across restarts
* A recent-connections list

[FRAMEWORK-COMPARISON.md](FRAMEWORK-COMPARISON.md) records why egui was chosen over gpui.

### Current limitations

* **CJK and emoji render as placeholder boxes.** The cell layout is correct — a wide
  character correctly reserves two columns, so surrounding text stays aligned — but egui's
  built-in monospace font has no glyphs for them. Fixing this means embedding a font that
  covers the ranges you need, at a cost in binary size.
* **The byte-oriented views do not soft wrap.** In ASCII, decimal and hex mode long lines
  scroll horizontally, because uniform row height is what lets the scrollback virtualize
  exactly. ANSI mode wraps properly, at the column boundary, because the emulator does it.
* **Selection is ANSI-mode only.** The ASCII, decimal and hex views allow selection within a
  row; use *Copy All* there.
* **ANSI mode holds the scrollback twice** — once as raw bytes, once as the emulator's grid.
  The raw ring has to exist so that switching display modes can replay it, and the two count
  scrollback in different units (bytes vs lines), so the line limit is derived from the byte
  budget as an estimate.
* The bundled monospace font is egui's built-in one. Embedding Fira Code needs the TTF
  added to `resources/`.

## Development

### Setup

1. Install Rust via [rustup](https://rustup.rs). The build needs Rust 1.85 or newer.
2. Clone this repo.
3. Run it:

  ```bash
  cargo run
  ```

There is no Node toolchain, no web build step and no separate frontend — it is one Rust
crate.

### Build a release binary

```bash
cargo build --release
```

The executable lands in `target/release/UniTerm.exe`. It is self-contained: the window and
executable icons are embedded at build time.

> Note: unlike the previous Tauri build, `cargo build` does not produce an installer. If you
> need MSI/NSIS packaging, add [`cargo-dist`](https://github.com/axodotdev/cargo-dist) or
> `cargo-wix`.

### Tests and lints

```bash
cargo test
cargo clippy --all-targets
```

### Logging

Logging is off by default apart from warnings. Turn it up with `RUST_LOG`:

```bash
RUST_LOG=uniterm=debug cargo run
```

### Layout

| Path | Contents |
|---|---|
| [src/main.rs](src/main.rs) | eframe entry point, tokio runtime, tracing setup |
| [src/app.rs](src/app.rs) | `eframe::App`, the dock, the tab viewer, the toolbar |
| [src/session.rs](src/session.rs) | Connection lifecycle and the async serial task |
| [src/settings.rs](src/settings.rs) | Port parameters, display and send modes |
| [src/discovery.rs](src/discovery.rs) | Serial port enumeration |
| [src/term/mod.rs](src/term/mod.rs) | The raw byte ring, and why it is the source of truth |
| [src/term/emu.rs](src/term/emu.rs) | Terminal emulator wrapper |
| [src/term/render.rs](src/term/render.rs) | Grid and byte-view rendering |
| [src/term/input.rs](src/term/input.rs) | Key and modifier to byte-stream mapping |
| [src/term/palette.rs](src/term/palette.rs) | Colour resolution |
| [src/term/text.rs](src/term/text.rs) | ASCII-view text preparation |
| [src/ui.rs](src/ui.rs) | Per-tab controls |
