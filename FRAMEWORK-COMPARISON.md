# egui vs gpui for UniTerm

Companion to [PLAN.md](PLAN.md). Evaluates `gpui` (Zed's UI framework) against
`egui` + `egui_dock` for the six planned features. All facts verified 2026-07-27.

## Verdict

**Stay with egui + `egui_dock`.**

The decision does not hinge on which framework is better in the abstract — gpui is the more
capable framework, and for a text-heavy app that matters. It hinges on three specific facts
about this project:

1. **gpui cannot currently be consumed from crates.io as its own README documents.** The
   published `gpui` is **0.2.2, dated 2025-10-22** — nine months stale. Current gpui has
   been split into `gpui` + `gpui_platform`, and **`gpui_platform` is not published at all**
   (crates.io returns 404). Real-world usage means pinning a **git revision of the Zed
   monorepo**.
2. **The two gpui features that would justify switching are GPL-3.0.** Zed's dock/panel
   system (`workspace`), terminal (`terminal`) and terminal view (`terminal_view`) are all
   `GPL-3.0-or-later`. UniTerm is **Apache-2.0** ([LICENSE](LICENSE)). You cannot lift that
   code without relicensing the whole app to GPL-3.0.
3. **A forkable, permissively-licensed reference exists for egui and not for gpui.**
   `egui_term` (MIT, ~1,800 LOC) is already "alacritty_terminal + egui renderer" — exactly
   [Task 6](PLAN.md#task-6--full-ansi-escape-sequence-rendering). The equivalent for gpui is
   Zed's own terminal, which is GPL.

Net effect on the plan: gpui costs roughly **20–35% more work** (~48–70 d vs ~40–55 d) with
materially higher variance, in exchange for a higher quality ceiling this app doesn't need.

---

## Verified facts

| | egui stack | gpui stack |
|---|---|---|
| Core crate | `eframe` **0.35.0** (2026-06-25) | `gpui` **0.2.2** (2025-10-22) |
| On crates.io, current? | Yes | **No — 9 months stale** |
| Platform crate | n/a (in `eframe`) | `gpui_platform` — **unpublished (404)** |
| Practical dependency form | crates.io, semver | **git rev of `zed-industries/zed`** |
| Framework license | MIT / Apache-2.0 | Apache-2.0 |
| Docking | `egui_dock` **0.20.1** (2026-06-28), MIT, targets egui 0.35 | none in core |
| Docking, third-party | — | `gpui-component` **0.5.1** (2026-02-05), Apache-2.0 |
| Docking, in Zed | — | `workspace` crate — **GPL-3.0-or-later** |
| Terminal widget | `egui_term` 0.1.0 (2025-04-24), **MIT**, ~1.8k LOC | — |
| Terminal, in Zed | — | `terminal`, `terminal_view` — **GPL-3.0-or-later** |
| ANSI colour helper | `egui_sgr` 0.3.0 (2026-06-23), 34k downloads | — |
| Async runtime | none of its own; you own `main()` | own executor (`scheduler`, `async-task`, `async-channel`, `postage`); **no tokio** |
| Windows backend | winit, long-established | Win32 + DirectWrite; last platform added |
| Docs | extensive, live demo, large example corpus | "the best way to learn about these APIs is to read the Zed source code or drop a question in the Zed Discord" |
| Stability statement | pre-1.0, regular breaking releases | "still pre-1.0. There will often be breaking changes between versions." |

Both frameworks are pre-1.0 and both will break your build on upgrades. That is a wash and
not a differentiator.

Note the ecosystem workaround for fact #1: a third party has republished gpui's internal
monorepo crates as `open-gpui-*` (0.2.0, 2026-07-07) — `open-gpui-scheduler`,
`open-gpui-sum-tree`, `open-gpui-collections` and a dozen more — specifically so gpui's
dependency tree can be resolved outside the Zed repo. That such a fork exists, three weeks
old and from an unaffiliated maintainer, is itself the clearest signal about how settled
gpui's packaging is.

---

## Dimension by dimension, against this app's needs

### 1. Docking and tab layout — [Task 1](PLAN.md#task-1--convert-to-an-egui-app-with-egui_dock)

**egui.** `egui_dock` 0.20.1 is MIT, on crates.io, targets egui 0.35 (matching current
`eframe` 0.35), has ~4.5M downloads, and does precisely what you asked for — splits, tab
drag-and-drop, floating windows. Its `serde` feature serializes `DockState`, which is also
most of [Task 4](PLAN.md#task-4--save-state-on-window-close).

**gpui.** Nothing in core. Two paths:
- Zed's `workspace` crate — the real thing, battle-tested, and **GPL-3.0-or-later**. Off the
  table unless you relicense.
- `gpui-component` (Apache-2.0, 0.5.1) — includes "Dock layout for panel arrangements,
  resizing". Legitimately viable. But it stacks a second pre-1.0 dependency on top, and its
  own README depends on gpui via `git = "https://github.com/zed-industries/zed"`, so you
  inherit a transitive git pin and must match whatever Zed revision it expects. Also verify
  whether its dock state is serde-serializable before committing — if not, Task 4 gets more
  expensive.
- Or build docking yourself: weeks, not days.

**Edge: egui, clearly.** Same feature, one dependency, semver, and it hands you layout
persistence for free.

### 2. Terminal grid rendering — [Task 6](PLAN.md#task-6--full-ansi-escape-sequence-rendering), the hardest task

This is the dimension where gpui has a real argument, so it deserves a fair hearing.

**gpui's genuine advantage.** It is built by people building a code editor. GPU-accelerated
glyph atlas, DirectWrite/font-kit shaping, `ShapedLine`/`TextRun` with per-run styling, and
an architecture proven to handle editor-scale text with selection at 60fps. Zed's terminal
demonstrates the ceiling. If "best-in-class terminal rendering" were the product thesis,
gpui would be the right tool.

**Why it doesn't win here.**
- **You can't use the proof.** Zed's `terminal` and `terminal_view` are GPL-3.0-or-later.
  Reading them to understand the architecture is fine; copying code or closely-derived
  structure into an Apache-2.0 app is not. So gpui's flagship terminal is a demo you can
  admire, not a starting point you can fork.
- **egui has the forkable one instead.** `egui_term` is MIT, ~1,800 LOC, and already wires
  `alacritty_terminal` to an egui renderer. It's stale (0.1.0, April 2025, so roughly the
  egui 0.31 era) and PTY-oriented rather than byte-stream-oriented, so it needs an egui
  version bump and rework to accept serial/SSH bytes instead of spawning a shell. But as a
  reference for the renderer, cell metrics and input mapping — the parts my estimate said
  were hard — a permissively-licensed 1,800-line head start is worth real days.
- **You write the renderer yourself in both cases.** My 12–13 d estimate for Task 6 assumed
  building the egui renderer over `alacritty_terminal` from scratch. gpui doesn't remove
  that work; it gives you better primitives to do it with, against much thinner docs.

**Edge: gpui on ceiling, egui on time-to-working.** For a solo developer, the documentation
gap plus the GPL fence cancels the capability advantage.

### 3. tokio integration — [Tasks 2 and 3](PLAN.md#task-2--ssh-transport-alongside-serial)

Both `russh` and `tokio-serial` are tokio-bound, so you need a tokio runtime either way.

**egui.** You own `main()`. Create the runtime, bridge with channels, call
`ctx.request_repaint()` when bytes arrive. Textbook, and heavily represented in community
examples.

**gpui.** Confirmed: gpui has **no tokio dependency** — it ships its own executor.
`Application::run` takes the main thread. You run tokio on its own threads and bridge into
gpui via `cx.spawn` / `AsyncApp` plus channels. Perfectly doable, just a second async world
to reason about and fewer worked examples to copy.

**Edge: egui, slightly.** Add ~1–2 d under gpui for the bridge.

### 4. Windows as the primary target

This matters more for UniTerm than for a typical app: it's a COM-port tool, the README
documents Windows UAC behaviour, and [main.rs:4-7](src-tauri/src/main.rs#L4-L7) already
carries the `windows_subsystem` attribute.

**egui.** winit-based, years of production Windows use across a large app population.

**gpui.** Win32 + DirectWrite, no feature flags needed, and Zed does now offer Windows
downloads. But Windows was the last platform gpui gained and remains the least-exercised of
the three.

**Edge: egui.** Not a claim that gpui is broken on Windows — a claim about where the
long-tail bug risk sits when Windows is your only shipping target.

### 5. Persistence — [Task 4](PLAN.md#task-4--save-state-on-window-close)

**egui.** `eframe` ships it: `persistence` feature, `App::save`, `eframe::get_value` /
`set_value`, auto-written to `%APPDATA%\<app>\app.ron`, plus `egui_dock`'s `serde` feature
for layout. This is why I sized Task 4 at 2.5–3 d.

**gpui.** No built-in application-state persistence — Zed's lives in its own (GPL) crates.
You hand-roll the config file, the schema, the save trigger and the restore path.

**Edge: egui.** Task 4 grows to roughly 4–5 d under gpui.

### 6. Build and iteration cost

**egui.** `eframe` is a modest tree. Incremental builds in the ~30–60 s range.

**gpui.** Pulls `wgpu`, `lyon`, `resvg`/`usvg`, `taffy`, `accesskit`, plus the Zed monorepo's
internal crates. Cold builds run to many minutes; binaries are larger. If you also take
`gpui-component`, more again.

**Edge: egui.** Compounding, since iteration speed is already the main thing lost in leaving
Vite HMR behind.

### 7. Learning curve

**egui.** Immediate mode, one obvious way to do things, excellent docs and a live demo. For
an app of this size the mental model fits in your head.

**gpui.** Hybrid immediate/retained with `Entity<T>`, `Context<T>`, the `Render` trait and
element lifecycles — more powerful and more to learn. The official learning path is reading
Zed's source, which is a large GPL codebase.

**Edge: egui**, decisively, for a solo developer on a side project.

### 8. Visual ceiling

**gpui** is far nicer: real shaping, smooth animation, proper subpixel text, Zed-grade
polish. Worth noting honestly.

But the current UI is plain Tailwind form controls, so egui is not a downgrade from where
you are — and for a terminal grid you specifically *don't* want clever text shaping.
[PLAN.md](PLAN.md#task-6--full-ansi-escape-sequence-rendering) already calls for disabling
Fira Code's ligatures so cell metrics stay valid, which neutralises much of the advantage
for the one view that matters most.

**Edge: gpui, but the app doesn't cash it in.**

---

## The licensing trap

Worth stating plainly because it's the least obvious finding and it inverts the intuitive
case for gpui.

The intuitive argument is: *"Zed is a code editor with a great built-in terminal and a
great docking system, all in gpui — so gpui must be the shortcut for a docked terminal
app."* The code exists and it's public.

But the licenses split the repo:

| Zed crate | Purpose | License |
|---|---|---|
| `gpui` | the framework | **Apache-2.0** |
| `workspace` | dock / panel system | **GPL-3.0-or-later** |
| `terminal` | terminal emulation | **GPL-3.0-or-later** |
| `terminal_view` | terminal rendering | **GPL-3.0-or-later** |

So the framework is permissive and every reusable-looking application-level component is
copyleft. UniTerm is Apache-2.0. Taking any of those three means UniTerm becomes
GPL-3.0-or-later — which affects redistribution of your binaries and anyone who forks the
project.

Two legitimate responses:

- **Relicense to GPL-3.0 deliberately** and vendor Zed's terminal as a starting point. This
  is a genuine option and would make gpui competitive on effort. It's a licensing decision
  for you to make, not a technical one — and it's irreversible in practice once
  contributions land.
- **Stay Apache-2.0**, which is what I assume, and in which case gpui's headline advantages
  are visible but unusable and you're writing the dock (or trusting `gpui-component`) and
  the terminal yourself anyway.

---

## Revised estimates under gpui

| # | Task | egui | gpui | Why the difference |
|---|------|------|------|--------------------|
| 1 | Port + docking | 11–14 d | **16–22 d** | Steeper model, thin docs, docking via extra dep or DIY, longer builds |
| 2 | SSH transport | 9–11 d | **10–13 d** | +tokio↔gpui executor bridge |
| 3 | Reconnect | 4–5 d | **5–6 d** | Mostly framework-independent |
| 4 | Persistence | 2.5–3 d | **4–5 d** | No `eframe`-style built-in storage |
| 5 | Recents | 2–3 d | 2–3 d | Framework-independent |
| 6 | ANSI terminal | 12–13 d | **14–18 d** | Better primitives, no forkable reference, sparse docs |
| | **Total** | **40–55 d** | **48–70 d** | |

Variance is also worse under gpui: a git-pinned monorepo dependency, an unpublished platform
crate, and "ask in Discord" as the documented support channel are all schedule risks that
don't have clean mitigations.

---

## A third option you should at least price

Since [Task 6](PLAN.md#task-6--full-ansi-escape-sequence-rendering) is the largest single
line item at 12–13 d, and since the webview you already have does most of it for free:

**Keep Tauri and use xterm.js.** It gives you the grid, full ANSI/VT handling, selection,
copy, reflow, alternate screen and link detection — battle-tested in VS Code — and the
webview keeps free text selection, clipboard, find-in-page and IME, all of which
[PLAN.md](PLAN.md#task-1--convert-to-an-egui-app-with-egui_dock) flags as regressions of any
native port. Task 6 collapses to roughly 2–3 d of wiring bytes into a `Terminal` instance.
Docking comes from any of several mature JS libraries. Rough total for all six features:
**~15–22 d**, less than half either native path.

Costs: you stay on the Node/Svelte toolchain, you still owe the Tauri
2.0-beta → stable migration ([Cargo.toml:12-25](src-tauri/Cargo.toml#L12-L25)), and you keep
shipping a webview.

I'm not recommending it over the native port — there are good reasons to want a single
language, no Node toolchain, a smaller binary and no webview dependency, and you've already
signalled that direction. But it is the cheapest route to your stated feature list by a wide
margin, so it should be a decision rather than an omission.

---

## When gpui would be the right call

Switch if **most** of these hold:

- Terminal rendering quality is the product's differentiator — huge scrollback, 60fps
  smoothness, beautiful text — rather than a means to talking to serial devices.
- You're willing to relicense UniTerm as GPL-3.0-or-later and vendor Zed's terminal.
- macOS and Linux matter as much as Windows.
- You can absorb pre-1.0 churn on a git-pinned monorepo revision, plus long build times.
- You enjoy learning a framework by reading a large codebase, and the extra ~10–15 days is
  a feature of the project rather than a cost.

That describes a different project than "serial terminal tool that also does SSH." For the
project in [PLAN.md](PLAN.md), egui + `egui_dock` is the better fit — not because it's the
better framework, but because it's the one whose ecosystem hands you the docking system, the
persistence layer and a forkable terminal renderer under licenses you can actually use.

### If you do go gpui, verify these first

Cheap checks, in order, before committing any real time:

1. Build a hello-world on **Windows** pinned to the exact Zed revision `gpui-component`
   expects. Confirm it compiles and note the cold-build time.
2. Confirm `gpui-component`'s Dock is serde-serializable, or accept that Task 4 grows.
3. Prototype the tokio↔gpui bridge: `tokio-serial` reading real bytes into a gpui view.
4. Render 50,000 lines of scrollback with per-cell colour and confirm the frame time, since
   this is the whole reason to prefer gpui.
