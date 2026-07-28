# UniTerm Roadmap Plan

Planning document for six proposed features. Written against the tree at commit `6166a10`
(Tauri 2.0-beta + SvelteKit 4, ~1,000 LOC Rust / ~900 LOC Svelte).

> **Status: all six tasks are implemented** on branch `feat/egui-port`. The estimates and
> analysis below are kept as written, so they can be compared against what actually happened.
> Corrections found during implementation:
>
> * **eframe 0.35's `persistence` feature works.** An initial report that it was unbuildable
>   (needing an unpublished `ron 0.12.2`) was a stale local registry index, not a real conflict.
> * **egui 0.35 is a much larger API break than assumed.** `App::update(ctx)` became
>   `App::ui(&mut Ui)`, `TopBottomPanel`/`SidePanel` merged into one `Panel`, and `ctx.style()`
>   became `style_of(theme)`.
> * **russh is not toolchain-free by default.** Its default `aws-lc-rs` backend needs CMake and
>   NASM and does not build on a stock Windows toolchain; the `ring` backend does. This
>   contradicted the "pure Rust, no C bindings" assumption the framework choice was made on.
> * **Saved state must not be JSON.** `DockState` holds `egui::Rect`s initialised to infinity,
>   which JSON cannot represent; RON is required. This is why eframe uses RON internally.
> * **SSH was testable after all.** Task 2's "no way to verify without a server" assumption was
>   wrong: russh has a server side, so the client is tested end to end in-process.

- [Summary](#summary)
- [Where the current design stands](#where-the-current-design-stands)
- [Recommended ordering](#recommended-ordering)
- [Task 1 — Convert to an egui app with egui_dock](#task-1--convert-to-an-egui-app-with-egui_dock)
- [Task 2 — SSH transport alongside serial](#task-2--ssh-transport-alongside-serial)
- [Task 3 — SSH reconnect button](#task-3--ssh-reconnect-button)
- [Task 4 — Save state on window close](#task-4--save-state-on-window-close)
- [Task 5 — Recent connections list](#task-5--recent-connections-list)
- [Task 6 — Full ANSI escape sequence rendering](#task-6--full-ansi-escape-sequence-rendering)
- [Target module layout](#target-module-layout)
- [What survives the port](#what-survives-the-port)
- [Cross-cutting risks](#cross-cutting-risks)

---

## Summary

| # | Task | Size | Est. | Depends on | Headline risk |
|---|------|------|------|-----------|---------------|
| 1 | Convert to egui + `egui_dock` | **Large** | 11–14 d | — | Loses installer/bundler; loses free text-selection & copy |
| 2 | SSH transport | **Large** | 9–11 d | 1 (soft), 6 (soft) | Host-key verification is a real security design task |
| 3 | SSH reconnect button | **Medium** | 4–5 d | 2 | Scrollback is currently cleared on connect *by design* |
| 4 | Save state on close | **Small–Med** | 2.5–3 d | 1 (strongly) | Auto-reconnecting serial at startup grabs the wrong device |
| 5 | Recent connections | **Small** | 2–3 d | 4 | Temptation to persist credentials |
| 6 | Full ANSI rendering | **Large** | 12–13 d | 1 | Replaces the entire display pipeline, not an add-on |

**Total ≈ 40–55 engineer-days (~2.5–3 months solo)**, assuming one developer who knows Rust
and is learning egui. Estimates are implementation + manual test, excluding release
engineering beyond what's noted in Task 1.

Two things dominate the plan and are worth deciding before any code is written:

1. **Tasks 1 and 6 are the same task wearing two hats.** The current display pipeline is
   HTML-string based — Rust converts ANSI to `<div>` soup and the webview does layout,
   wrapping, scrolling and selection. egui has no HTML. So porting to egui *requires*
   writing a new renderer, and the only renderer worth writing twice is the real one.
   Doing Task 1 with a placeholder text view and Task 6 later means building the terminal
   view two times.
2. **Tasks 4 and 5 are nearly free on egui and expensive on Tauri.** `eframe` ships
   persistence (`App::save` + `eframe::get_value/set_value`, auto-saved to
   `%APPDATA%\<app>\app.ron`) and `egui_dock`'s `serde` feature serializes `DockState`
   directly. Building the same thing on Tauri means a window-close hook, a hand-rolled
   JSON file, and Svelte store rehydration — all of it deleted by Task 1.

---

## Where the current design stands

Facts that shape every estimate below.

**Display is HTML strings, not a terminal.** [ansi_to_html.rs](src-tauri/src/ansi_to_html.rs)
emits `<div style="color:rgb(...)">` per text run, and [PortView.svelte:66](src/lib/PortView.svelte#L66)
injects it with `{@html port.rx_buffer}`. Line breaks are faked with a zero-height flex
div. There is no cursor, no grid, no screen — `AnsiSequence::CursorPos` is parsed and
discarded at [ansi_to_html.rs:83](src-tauri/src/ansi_to_html.rs#L83), and `EraseDisplay`
just empties the output vector.

**ANSI mode re-renders the entire scrollback on every read.** In
[serial.rs:212-220](src-tauri/src/serial.rs#L212-L220), `DisplayMode::Ansi` calls
`package_output()` — which re-parses and re-serializes all of `output` (default 20,000
bytes, slider goes to 2 MB) — on every 10 ms poll tick that produced data. O(n) per chunk.
A grid model fixes this by construction, since the emulator is fed incrementally.

**SGR coverage is partial.** [graphics_mode_class](src-tauri/src/ansi_to_html.rs#L115)
handles SGR 0–9, 22–29, 30–47 and 90–107. Missing: 256-colour (`38;5;n`), truecolour
(`38;2;r;g;b`), cursor motion, erase-in-line, scroll regions, DEC private modes, OSC,
alternate screen buffer. Also a live bug: bright yellow (`93`) and bright-yellow-background
(`103`) are both mapped to green at
[lines 152](src-tauri/src/ansi_to_html.rs#L152) and [160](src-tauri/src/ansi_to_html.rs#L160).

**Real-time keyboard input is ASCII-only.** [asciiCodes.ts](src/lib/asciiCodes.ts) is a
flat key-name → byte map. There are no arrow keys, no F-keys, and no Ctrl combos —
Ctrl+C yields `"c"` → 99, not `0x03`. Interactive remote shells will be unusable until
this is replaced (Task 6).

**Scrollback lives in two places and is cleared on connect.** Rust owns
`Port::output: Vec<u8>` ([port.rs:34](src-tauri/src/port.rs#L34)); JS owns
`Connection.rx_buffer: string` ([stores.ts:45](src/stores.ts#L45)). Both
`setIsActive` and `setIsRunning` reset `rx_buffer = ''`
([stores.ts:106](src/stores.ts#L106), [stores.ts:114](src/stores.ts#L114)), and
`Port::new` allocates a fresh buffer per connection. Task 3 explicitly requires the
opposite behaviour.

**One OS thread and one tokio runtime per connection.** [background.rs:63](src-tauri/src/background.rs#L63)
does `thread::spawn` into a `#[tokio::main]` fn, then races the session against a stop
signal and a port-presence poller via `tokio::select!`. It works, but it's a whole runtime
per port. The egui port needs a shared runtime anyway (eframe's main loop is synchronous),
so this collapses into one multi-thread runtime.

**Liveness detection is serial-specific.** [check_port_present](src-tauri/src/port_list.rs#L59)
polls `available_ports()` every 500 ms and errors when the port disappears. SSH has no
analog — Task 3 needs keepalives instead.

**Latent issues, unrelated to the six tasks but worth folding in.**
- [serial.rs:192](src-tauri/src/serial.rs#L192): `Ok(0) => break` silently ends the
  session task and returns `Ok(())`. The UI is never told; the tab still shows connected.
- Tauri deps are pinned to `2.0.0-beta` ([Cargo.toml:12-25](src-tauri/Cargo.toml#L12-L25))
  while `package.json` claims 2.0.0. `@tauri-apps/cli` appears as `^1.4.0` in
  `dependencies` *and* `>=2.0.0-beta.0` in `devDependencies`
  ([package.json:17](package.json#L17), [package.json:33](package.json#L33)). Staying on
  Tauri means a beta→stable migration; Task 1 deletes this problem instead.
- `Connection.name` / `Connection.baud_rate` in [state.rs:15](src-tauri/src/state.rs#L15)
  are `#[allow(dead_code)]`; `is_running` in the Svelte store is effectively unused.

**A note on the reference you gave.** `github.com/anhosh/egui_dock` is live and is the
maintained home of the project (originally `@lain-dono`, long maintained under
`Adanos020`, which is still what crates.io metadata points at). Same crate either way:
`egui_dock` **0.20.1**, which targets **egui 0.35** — matching current `eframe` **0.35.0**.
Versions line up today; see [Cross-cutting risks](#cross-cutting-risks) about keeping them
that way.

---

## Recommended ordering

```
  1. egui + egui_dock  ─────┬──── 4. persist state ──── 5. recent connections
     (foundation)           │
                            └──── 6. ANSI/terminal core ──── 2. SSH ──── 3. reconnect
```

Rationale:

- **1 first, always.** Every other task is cheaper in egui, and three of them touch code
  that Task 1 deletes.
- **6 fused with 1, not after it.** Build the terminal grid renderer *as* Task 1's
  `PortView` replacement. Splitting them means writing the view twice.
- **2 after 6.** SSH without cursor addressing and colour means no `vim`, no `top`, no
  coloured prompt — a demo, not a feature. ANSI is optional for serial and mandatory for
  SSH.
- **3 after 2**, since it's a state machine over the SSH session.
- **4 and 5 anytime after 1**, and they're a good warm-up: small, self-contained, and they
  force you to settle the config-schema question early rather than retrofitting it.

If you want something shippable sooner: **1 + 6 is a coherent release on its own** ("UniTerm
3.0: native, real terminal emulation"), with 2/3/4/5 as a follow-up. Shipping 1 alone is
not advisable — it would be a feature *regression* (see the selection/copy note in Task 1).

---

## Task 1 — Convert to an egui app with egui_dock

**Size: Large — 11–14 days.**

### Work breakdown

| Item | Est. |
|------|------|
| Restructure crate: drop `tauri`/`tauri-build`, add `eframe` 0.35 + `egui_dock` 0.20 | 0.5 d |
| Runtime bridge: shared tokio runtime, `mpsc` event channel, `ctx.request_repaint()` on data | 1 d |
| Replace `app.emit("serial", …)` with typed channel sends; strip `#[command]` / `State` / `AppHandle` | 1–1.5 d |
| Rebuild `PortMenu`: 6 combo boxes, scrollback slider, connect/disconnect/remove, log controls, send row | 2 d |
| Rebuild `PortView` — **do this as Task 6's renderer** | (see Task 6) |
| `egui_dock` integration: `DockState<TabId>`, `TabViewer` impl, add/close, toolbar | 1–1.5 d |
| File dialog: `tauri-plugin-dialog` → `rfd` | 0.25 d |
| Icons: `@iconify/svelte` has no egui equivalent — embed an icon font or use text labels | 0.5 d |
| Fonts: embed Fira Code via `egui::FontDefinitions` | 0.25 d |
| Packaging: replace `cargo tauri build` (MSI/NSIS/dmg/deb) | 1–2 d |
| Delete `src/`, `package.json`, vite/svelte/tailwind config; rewrite README dev setup | 0.5 d |

### Issues this introduces

**Text selection and copy stop being free — this is the big one.** The webview gave you
click-drag selection across the whole terminal output, Ctrl+C, and browser find. In egui
you get `Label::selectable(true)` per-galley; stream selection spanning a virtualized
scroll area is something you implement. Users will notice on day one. Budget it in Task 6
(it's listed there) and do not ship Task 1 without it.

**Text layout performance becomes your problem.** The DOM virtualized nothing but was
fast anyway; egui builds a galley per text block per frame unless cached. A 2 MB monospace
scrollback cannot be laid out per frame — you need `ScrollArea::show_rows` over a
line-indexed model so only visible rows are built. Skip this and the app hitches badly at
large scrollback settings. Non-negotiable, not an optimisation.

**You lose the Tauri bundler.** `cargo tauri build` produced platform installers with icon
embedding for free. eframe produces a bare executable. Options: `cargo-dist` (best
multi-platform story), `cargo-wix` or NSIS for a Windows MSI, or ship a zipped `.exe` and
accept the downgrade. Also carry over
`#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]`,
already present in [main.rs:4-7](src-tauri/src/main.rs#L4-L7), or every launch flashes a
console.

**Iteration loop gets slower.** Vite HMR (sub-second) is replaced by incremental Rust
builds (~30–60 s for an eframe app). This changes how UI tweaking feels more than people
expect.

**Styling must be hand-translated.** Tailwind classes and the `.selected` tab CSS become
`egui::Style` / `Visuals` code. Mechanical, but it's the whole visual design re-expressed
in Rust.

**Weaker IME and accessibility.** The webview handled IME, non-ASCII composition and
exposed an a11y tree. egui has AccessKit but it's a step down; IME support for CJK input
is materially weaker.

### Design decision to make up front

Today every pane renders **all** ports as tabs — [TabView.svelte](src/lib/layout/TabView.svelte)
iterates the global `$ports` store, so the four fixed layouts are N independent tab
selectors over one shared list, and the same port can be visible in two panes
simultaneously. In `egui_dock`, a tab lives in exactly one node. Arbitrary splits and
drag-and-drop are a superset of the four fixed layouts, but "same connection visible
twice" is lost unless you allow duplicate tabs that share one connection ID. Decide
which semantics you want before writing the `TabViewer` — retrofitting shared-connection
tabs later means reworking your tab identity type.

---

## Task 2 — SSH transport alongside serial

**Size: Large — 9–11 days.**

Recommended crate: **`russh` 0.62.4** — pure Rust, tokio-based, no C toolchain. The
alternative, `ssh2` (libssh2 FFI), is more battle-tested but blocking and needs a C
compiler plus vendored OpenSSL on Windows, which fights the current build story.

### Work breakdown

| Item | Est. |
|------|------|
| `Transport` trait (open / read / write / close) over serial + SSH; refactor the `serial.rs` loop into a transport-agnostic session loop | 2 d |
| russh client: `Handler` impl, password + public-key auth, `channel_open_session`, `request_pty`, `request_shell`, consume `ChannelMsg::Data` | 3 d |
| SSH settings UI: host, port, user, auth method, key path, passphrase | 1 d |
| `ConnectionKind` enum replacing the flat `PortSettings`; ripples through `state.rs`, `background.rs`, `message.rs`, UI | 1 d |
| `known_hosts` trust store + trust-on-first-use prompt | 1–1.5 d |
| `window_change` on pane resize so remote apps reflow | 0.5 d |
| Credential handling (memory-only default, optional `keyring`) | 1 d |

### Issues this introduces

**Host-key verification is a security design task, not a checkbox.** russh hands you
`Handler::check_server_key` and does what you tell it. Returning `Ok(true)` — the shape
every tutorial uses — makes every connection MITM-able. You need a real `known_hosts`
store, a first-connection prompt showing the fingerprint, and a loud failure when a known
host's key changes. Plan it as part of the feature, not a hardening pass afterward.

**Credential storage is where this goes wrong quietly.** Tasks 4 and 5 will make it very
tempting to write passwords and key passphrases into the config file. Default to storing
nothing; hold secrets in memory for the session's lifetime only; if you want persistence,
use the OS keychain via the `keyring` crate. A plaintext password in
`%APPDATA%\UniTerm\app.ron` is the kind of thing that's discovered later and is
embarrassing.

**Serial settings are meaningless for SSH.** Baud rate, parity, data bits and stop bits
must disappear (not grey out) for SSH tabs, or the UI reads as nonsense. This is why
`ConnectionKind` needs to be a proper enum rather than an extra field on `PortSettings` —
otherwise the kind-awareness leaks into every widget.

**Liveness has no serial analog.** `check_port_present` polling `available_ports()` doesn't
translate. Add SSH keepalive (periodic global request, or track `ChannelMsg::Eof` /
`Close` plus a read timeout) — and note that without this, Task 3's reconnect button has
nothing to react to, because a silently dropped TCP connection looks identical to an idle one.

**ANSI stops being optional.** Serial devices often emit plain text. Remote shells emit
escape sequences constantly — prompts, `ls` colours, cursor motion. SSH tabs will look
like garbage until Task 6 lands, which is why the ordering puts 6 first.

**Display modes need thought.** Decimal and Hex over an interactive shell are odd but
harmless. Keep them available (they're useful for debugging the transport itself) and let
ANSI be the default for SSH connections.

**`russh` moves fast.** Three patch releases in the two weeks before this was written
(0.62.2 → 0.62.4), pre-1.0, with breaking changes between minors. Pin the exact version
and expect a small migration each time you bump.

---

## Task 3 — SSH reconnect button

**Size: Medium — 4–5 days.**

Requirements as stated: a button the user presses; failed reconnects reset the button so it
can be retried; reconnecting must **not** clear the existing terminal window.

### Work breakdown

| Item | Est. |
|------|------|
| Per-session state machine: `Disconnected → Connecting → Connected → Reconnecting → Failed` | 1 d |
| Button with in-flight state, disabled while connecting, reset to enabled on failure | 0.5 d |
| **Decouple scrollback lifetime from connection lifetime** | 1 d |
| Keepalive / drop detection so a dropped connection is noticed at all | 1 d |
| Optional auto-retry with backoff (recommended as opt-in, off by default) | 0.5 d |

### Issues this introduces

**The "don't clear the terminal" requirement is the inverse of current behaviour, in three
places.** `setIsActive` and `setIsRunning` both do `rx_buffer = ''`
([stores.ts:106](src/stores.ts#L106), [stores.ts:114](src/stores.ts#L114)), and
`Port::new` allocates a fresh `output: Vec::new()`
([port.rs:49](src-tauri/src/port.rs#L49)). Buffer ownership has to move up a level, so
the buffer belongs to the *tab* and the transport is a thing that gets attached to it.
That's a small refactor after Task 1 and a wasted one before it.

**Terminal state must reset without clearing scrollback.** The remote's screen state is
gone after a drop. When the new session starts writing, cursor position, scroll region,
SGR attributes and alt-screen state must all reset — while the scrollback stays on screen.
In a grid model that's "reset cursor + attributes + modes, retain scrollback". Emit a
divider line (`—— reconnected 14:02:11 ——`) so the user can see where the seam is; it's
also the cheapest way to make the feature legible.

**Reconnect must be idempotent.** Disable the button in the state machine, not just
visually — a double-click or an Enter-key repeat during `Reconnecting` must not open two
sessions. Related: fully drop the old russh session and channel handles before
reconnecting, or you leak a tokio task and a socket per attempt.

**Password auth can't reconnect silently.** If the credential wasn't persisted (the
correct default), a reconnect either prompts again or uses a session-lifetime in-memory
copy. Recommend the in-memory copy: it makes the button work as users expect without
writing secrets to disk. Say so in the UI.

**Worth extending to serial.** The same button, backed by the existing
`check_port_present` signal, handles the very common "USB-serial adapter got unplugged and
replugged" case. Small increment on top of this task, and probably the more frequently used
half of the feature. Note that on Windows a replugged adapter may come back on a
*different* COM port, so serial reconnect should optionally match on USB
serial-number/VID:PID rather than port name — [port_list.rs](src-tauri/src/port_list.rs)
already collects `serial_number` and `manufacturer`.

---

## Task 4 — Save state on window close

**Size: Small–Medium — 2.5–3 days**, *provided Task 1 is done first.*

On Tauri this would be a window-close handler, a hand-rolled JSON file and Svelte store
rehydration — roughly twice the work, all of it deleted by Task 1.

### Work breakdown

| Item | Est. |
|------|------|
| Enable `eframe` `persistence`; implement `App::save` with `eframe::set_value` | 0.5 d |
| `DockState` serde round-trip (`egui_dock`'s `serde` feature — it exists and is exactly this) | 0.5 d |
| Decide and implement the saved schema | 0.5 d |
| Restore on startup, including the auto-connect decision below | 1 d |
| Schema versioning + graceful reset on corrupt/incompatible config | 0.5 d |

**Save:** dock layout, connection definitions (kind, port/host, all settings), display mode,
scrollback size, log paths and enabled flags, window geometry.
**Don't save:** scrollback contents (offer it as an explicit opt-in if wanted — it's a data
volume and a data-sensitivity question, since terminal output routinely contains secrets),
and credentials.

### Issues this introduces

**Auto-reconnecting serial ports at startup is genuinely risky.** COM port numbering is not
stable on Windows — `COM3` today can be a different physical device tomorrow. Auto-opening
it means writing to, or at minimum asserting control over, hardware the user didn't ask
you to touch, and it can steal a port another application wants. **Recommendation: restore
sessions as *defined but disconnected*,** with connect being one click, and auto-connect as
an explicit per-session opt-in. Where the settings include a USB serial number, verify it
matches before auto-connecting.

**Corrupt-config recovery matters more than it sounds.** A malformed or version-skewed
`app.ron` must not prevent startup. Version the schema, and on any deserialize failure log
it, back the file up, and start clean. Without this, one bad release bricks the app for
users and the only fix is "delete this file in AppData".

**eframe persistence has a crash caveat.** State is written on clean shutdown and
periodically (every 30 s by default, configurable via `App::auto_save_interval`). A crash
or a kill loses everything since the last autosave. If sessions-restore is meant to be
dependable, save on meaningful mutations (tab added/removed/moved) too, not just on close.

**Log-file paths may be stale.** A restored session can point at a path that's gone,
read-only, or on an unmounted drive. Validate lazily and surface the error in the tab
rather than failing the restore.

---

## Task 5 — Recent connections list

**Size: Small — 2–3 days.** Depends on Task 4's persistence layer; do it immediately after.

### Work breakdown

| Item | Est. |
|------|------|
| Ring buffer of recent connection descriptors, capped (~20), dedup by identity | 1 d |
| UI: dropdown on the existing "+" button, and/or a startup panel when no tabs are open | 1 d |
| "Reopen" = create tab from descriptor + connect; reuse Task 4's serialization | 0.5 d |
| Optional: pin/favourite, clear-history, rename entries | 0.5 d |

### Issues this introduces

**Dedup identity is a real decision.** Is `COM3 @ 9600` the same entry as `COM3 @ 115200`?
Recommendation: key on the full settings tuple so both appear — for a serial tool the baud
rate is part of what you're trying to remember, and collapsing them loses the useful bit.
For SSH, key on `user@host:port`.

**Same credential trap as Tasks 2 and 4.** A "recent connections" list whose entries
reconnect in one click is precisely where someone decides to store the password. Store a
reference (keyring entry name), never the secret.

**Mild information disclosure on shared machines.** A list of recent SSH hosts and
usernames in `%APPDATA%` is comparable to `known_hosts` and generally acceptable — worth
having a "clear history" affordance, and worth making sure passwords never reach the log
file that [tracing](src-tauri/src/main.rs#L32) currently writes at `TRACE` level.

**Startup panel is the highest-value half.** A launcher listing recents when no tabs are
open makes the feature discoverable; a menu buried on the "+" button mostly doesn't get
found. If time is short, build the panel and skip the pin/rename extras.

---

## Task 6 — Full ANSI escape sequence rendering

**Size: Large — 12–13 days** with the recommended approach.

This replaces [ansi_to_html.rs](src-tauri/src/ansi_to_html.rs) and
[PortView.svelte](src/lib/PortView.svelte) entirely. It is not an extension of the
existing code — a real terminal needs a screen grid, a cursor, scroll regions and an
alternate buffer, and the current design has none of those concepts.

### Two credible approaches

**(A) Roll your own on `vte` 0.15** — the parser Alacritty uses; you implement
`vte::Perform` and own the grid.

| Item | Est. |
|------|------|
| `Perform` impl + grid model + scrollback ring | 4 d |
| Full SGR incl. 256-colour and truecolour | 1.5 d |
| Cursor ops, erase, scroll regions, insert/delete lines & chars | 3 d |
| Line wrapping + resize reflow | 2 d |
| Alternate screen buffer | 1 d |
| egui renderer, virtualized rows, per-cell colour runs → `LayoutJob` | 3 d |
| Selection and copy | 2 d |
| Keyboard → escape sequences | 2 d |
| Conformance tests | 2 d |
| **Total** | **~20–22 d** |

**(B) Use `alacritty_terminal` 0.26 as the emulator; write only the renderer and input
mapping. ← recommended**

| Item | Est. |
|------|------|
| Wire `Term` + `Processor` to the transport | 2 d |
| egui renderer over `renderable_content()`, virtualized rows | 4 d |
| Input mapping: modifiers, application cursor keys, bracketed paste | 2 d |
| Selection and copy via alacritty's `Selection` | 1.5 d |
| Resize: `term.resize()` driven by pane rect ÷ char metrics | 1 d |
| Reconcile with the Ascii/Decimal/Hex modes (see below) | 1.5 d |
| Logging: write raw bytes, not rendered output | 0.5 d |
| **Total** | **~12–13 d** |

(B) buys xterm-level correctness — alt screen, scroll regions, OSC, truecolour, reflow —
from the same core Alacritty ships, for about 40% less work. Its costs: a heavier
dependency, pre-1.0 with breaking changes between releases, thin documentation, and an API
that assumes a PTY-ish event listener you'll need to adapt. A third option,
`termwiz`/`wezterm-term`, is richer still and also pre-1.0; no reason to prefer it here.

### Issues this introduces

**The display-mode model has to change shape.** Today Ascii/Ansi/Decimal/Hex are four
stateless formatters over one byte buffer, so switching modes just re-renders
([serial.rs:156-162](src-tauri/src/serial.rs#L156-L162)). A real emulator is *stateful* and
must see every byte in order — you can't hand it the tail of a stream and get a correct
screen. **Keep the raw byte ring as the source of truth and replay it through a fresh
emulator when the user switches into ANSI mode.** That one decision is what keeps this
tractable; without it, mode switching either breaks or forces you to run all four
pipelines concurrently.

**Fira Code's ligatures must be disabled.** Cell metrics assume one advance width per
cell; ligatures merge glyphs and break the grid. Disable the feature or pick a
non-ligature mono face.

**Wide and combining characters are manual work.** egui's text layout isn't cell-based, so
CJK double-width, combining marks and emoji need explicit width handling (`unicode-width`)
and per-cell or per-run positioning. Alacritty tracks the wide-char flags for you in
approach (B); you still have to honour them when drawing.

**Virtualization is mandatory, again.** 200 columns × 50,000 lines of scrollback means only
visible rows may become galleys. Shared constraint with Task 1 — build it once.

**This is what makes SSH usable.** Cursor addressing, colour and alt-screen are the
difference between "SSH works" and "SSH prints garbage". It also fixes the O(n)-per-chunk
re-render described earlier, and closes the arrow-keys/Ctrl-combo gap in
[asciiCodes.ts](src/lib/asciiCodes.ts) — which is a prerequisite for any interactive
remote program.

---

## Target module layout

```
uniterm/
  Cargo.toml              # workspace root; no more package.json / vite / svelte / tailwind
  src/
    main.rs               # eframe entry, tokio runtime, tracing init
    app.rs                # eframe::App impl, DockState<TabId>, TabViewer, toolbar
    persist.rs            # versioned config schema, save/restore          [Task 4]
    recents.rs            # recent-connection ring buffer                 [Task 5]
    session/
      mod.rs              # Session: buffer owner + transport slot + state machine [Task 3]
      transport.rs        # Transport trait                               [Task 2]
      serial.rs           # serial impl  (from src-tauri/src/serial.rs)
      ssh.rs              # russh impl                                    [Task 2]
      settings.rs         # ConnectionKind enum (from port_settings.rs)   [Task 2]
      discovery.rs        # port enumeration + presence (from port_list.rs)
      log.rs              # raw-byte file logging (from port.rs)
    term/
      mod.rs              # alacritty_terminal Term wrapper, raw byte ring [Task 6]
      render.rs           # virtualized egui renderer                     [Task 6]
      input.rs            # key/modifier → escape sequences               [Task 6]
      select.rs           # selection + clipboard                         [Task 6]
    ui/
      menu.rs             # from PortMenu.svelte
      options.rs          # from PortMenuOptions.ts
      launcher.rs         # recents panel                                 [Task 5]
```

## What survives the port

| Current file | Fate |
|---|---|
| [port_settings.rs](src-tauri/src/port_settings.rs) | **Keep** — pure enums + `tokio_serial` conversions, UI-agnostic. Extend for `ConnectionKind`. |
| [port_list.rs](src-tauri/src/port_list.rs) | **Keep** — drop `#[command]`, keep enumeration and presence polling. |
| [message.rs](src-tauri/src/message.rs) | **Mostly keep** — becomes internal channel messages; `MessageData`'s `#[serde(untagged)]` can go. |
| [serial.rs](src-tauri/src/serial.rs) | **Refactor** — session loop becomes transport-generic; `app.emit` → channel send; fix the `Ok(0) => break`. |
| [background.rs](src-tauri/src/background.rs) | **Simplify** — thread-per-port + per-port runtime collapses into tasks on one shared runtime. |
| [state.rs](src-tauri/src/state.rs) | **Rewrite** — drop `AppHandle`/`State`/`#[command]`; buffer ownership moves to `Session`. |
| [port.rs](src-tauri/src/port.rs) | **Split** — logging survives in `session/log.rs`; display formatting is superseded by `term/`. |
| [ansi_to_html.rs](src-tauri/src/ansi_to_html.rs) | **Delete** — replaced by `term/`. |
| [PortMenuOptions.ts](src/lib/PortMenuOptions.ts) | **Port** — the baud/parity/etc. option tables translate directly to Rust consts. |
| [asciiCodes.ts](src/lib/asciiCodes.ts) | **Replace** — superseded by `term/input.rs`, which must also cover arrows, F-keys and Ctrl. |
| Everything else in `src/` | **Delete.** |

## Cross-cutting risks

**Version coupling in the egui ecosystem.** `eframe` 0.35 / `egui_dock` 0.20.1 /
`alacritty_terminal` 0.26 / `russh` 0.62.4 are all pre-1.0 and all break APIs on minor
bumps. `egui_dock` in particular tracks egui releases with a lag, so an egui bump can be
blocked on it. Pin exact versions, commit `Cargo.lock`, and treat dependency upgrades as
scheduled work (~0.5–1 d per bump) rather than incidental maintenance.

**No test suite exists today.** Nothing in the tree is tested, and this plan rewrites the
display pipeline and adds a network transport. At minimum: unit tests for escape-sequence
handling (feed bytes, assert grid state), a config schema round-trip test, and a reconnect
state-machine test. Conformance tests are already line-itemed in Task 6(A) and worth
adding to (B) too — call it 2 d on top.

**Estimates assume the ordering above.** Building Tasks 3, 4 or 5 on the current Tauri
frontend means writing code against `stores.ts` and Svelte components that Task 1 deletes —
roughly double the effort for those three, plus the merge pain of a large in-flight port.

**The port is a feature regression until Task 6 ships.** Free text selection, copy,
find-in-page, IME and the accessibility tree all come from the webview. Shipping Task 1
alone would be visibly worse than 2.0 for everyday use. Treat 1 + 6 as the minimum
releasable unit.
