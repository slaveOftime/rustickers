# Rustickers frontend handbook

**Audience:** any contributor adding or improving a feature that touches
the desktop UI, theme, sticker rendering, clipboard surface, or system
windows.

**Why this doc exists:** the desktop app was migrated to **GPUI Kit
0.7.1** in October 2026. Every UI concern now flows through Kit, not
the legacy `gpui`/`gpui-component` Git pins. This handbook is the
catch-up packet for anyone returning to the codebase later: it tells
you where the kit layer lives, where the bridge to the legacy imports
still lingers, what theme tokens to use, what design rules already
govern the surface, and where to look for further reading.

> If you change something in this doc, also update the related source
> files. This file is the **first stop** for newcomers, not the only
> stop.

---

## 1. GPUI Kit 0.7.1 at a glance

### What the kit is

`gpui-kit` is the facade crate published on crates.io by
[longbridge/gpui-kit](https://crates.io/crates/gpui-kit). Everything
Rustickers uses for the desktop UI flows through it:

| Sub-module | Role |
| --- | --- |
| `gpui_kit::*` | Core GPUI surface: `App`, `Context`, `Window`, `Entity`, async primitives, geometry (`Rgba`, `Hsla`, `Size`, `Bounds`), input types (`MouseEvent`, …), action system. These are the **same** types the rest of GPUI uses; the kit re-exports them. |
| `gpui_kit::component::*` | Component layer: `Button`, `Input`, `Textarea`, `List`, `uniform_list`, `Dialog`, `Theme`, `ActiveTheme`, `Selectable`, `IconNamed`, … |
| `gpui_kit::base::*` | Underlying rendering primitives a few low-level references still need. |
| `gpui_kit::assets` | Asset registry. Default icons live here. |

### Pinned version, single family in the closure

We're pinned to `gpui-kit = "0.7.1"` and `gpui-wry = "0.7.1"` from
crates.io. The closure matches: **one** `gpui-pre 0.3.8` family.

Verify yourself any time:

```bash
cargo tree --features ui --edges normal | grep '^name = "gpui-'
```

If more than one `"gpui-pre …"` line shows up, **stop and fix**; the
migration's invariant is that the crate takes the kit's re-exported
presets so the door is closed on floating again.

Upstream HEAD still labels itself 0.7.1 in `Cargo.toml` but contains
unreleased changes (and, since v0.7.2, an additional `gpui-fast`
crate). Stay on 0.7.1; bump to a newer tag only after a fresh audit.

### The two namespace bridges still in `src/lib.rs`

```rust
#[cfg(feature = "ui")]
extern crate gpui_kit as gpui;

#[cfg(feature = "ui")]
pub mod gpui_component {
    pub use gpui_kit::component::*;
}

#[cfg(feature = "ui")]
extern crate gpui_wry;

#[cfg(feature = "ui")]
extern crate lb_wry as wry;
```

Why they stay:

- Rust submodule bare-name resolution only reaches **extern crates**
  or current-scope items. So `use gpui_component::…` in a submodule
  resolves to *something* only if it's an `extern crate` declaration
  in the crate root. These two bridges (plus the legacy `gpui` alias
  and the wry rename) make a file-by-file import migration possible
  without rewriting every consumer in lockstep.
- After the migration, **every** `use gpui_component::…` in the source
  tree has been rewritten to `use crate::gpui_component::…`. When all
  the bare-in-scope materials have moved off the bridges, remove them
  entirely and have each module import directly from `gpui_kit::*`.

How to grep for stragglers:

```bash
git grep -n 'use gpui_component::' src/ tests/ examples/  # expect 0 hits
git grep -n 'use gpui::'        src/                       # bridge uses only
```

---

## 2. Where things live

```
src/
  lib.rs                        ← namespace bridges for the migration
  rustickers.rs                 ← CLI / `SingleInstance` / glue
  model/sticker.rs              ← StickerColor (serialised identity; no UI literals)
  native/
    mod.rs                      ← `pub mod theme;`, defines the application name,
                                  threads the Kit theme / root plugin layer / assets
    theme.rs                    ← UI-owned StickerColor palette split (see §6)
    windows/
      main.rs                   ← manager window
      selection.rs              ← selection popup (clipboard chooser round 2)
      clipboard.rs              ← clipboard chooser
      clipboard/editor.rs       ← clipboard item editor surface
      sticker/open.rs           ← sticker window boot
      sticker/view.rs           ← sticky chrome (header, color picker, footer)
      sticker/mod.rs            ← sticker window internals
    components/
      webview.rs                ← WRY WebView lifecycle + per-monitor DPI handling
      stickers/
        markdown.rs             ← markdown note family
        content_lock.rs         ← password / lock dialog tokens
        timer.rs                ← timer family
        command/                ← sticky command-runner family
        file/                   ← file / audio / image family
        paint.rs                ← paint family with per-strokes data domain palettes
tests/
  clipboard_storage.rs          ← 18 privacy regression tests — DO NOT regress
  sticker_palette.rs            ← 3 StickerColor palette regression tests
.github/workflows/ci.yml        ← windows-latest / ubuntu-latest / macos-14
```

Adding a new family:

1. Add `mod <family>;` to `src/native/components/stickers/mod.rs` next
   to the existing ones. The export list mirrors how `markdown.rs` and
   `timer.rs` work — keep the per-sticker module flat and use shared
   helpers in `stickers/mod.rs`.
2. Pull the boilerplate through `crate::native::components::stickers::…`
   so `Background::busy()`, `FOCUS_LOSS_RELOCK_DELAY`, and similar
   shared services are reused.
3. Follow §5 component patterns and §8 spawn/cancel rules from the
   start.

---

## 3. Application bootstrap

The desktop app boots in `src/native/mod.rs` (the kit-native path,
under `#[cfg(feature = "ui")]`). The essentials:

```rust
let app = gpui::application();               // not Application::with_platform(…)

app.on_open_urls(...)                        // single-instance IPC hook
   .on_action(Command::new)                  // global hotkeys via Kit action system
   .with_assets(gpui_kit::assets::Assets)
   .run_with_launcher(…)
}
```

Three things to remember before adding windows:

1. **Use `gpui::application()`** — the legacy `Application::with_platform(gpui_platform::current_platform(false))`
   construct was removed in the migration. Reach for `gpui_kit::application`
   instead of re-creating the platform by hand.
2. **Don't override the theme background** at boot. The old forced-brown
   `cx.global_mut::<Theme>().background = rgb(0x151104)` is gone; light/dark
   theming now flows through Kit.
3. **Don't manually host the dialog layer**. `.children(Root::render_dialog_layer(window, cx))`
   was removed from both the manager render and the sticker window render.
   `gpui_kit::component::init(cx)` is called once during boot and the
   plugin model registers dialogs for you.

### Transparent surfaces

For windows whose content paints its own background — e.g., sticker
windows with transparent corners — pass `WindowBackground::Transparent`
when constructing `WindowOptions`. Don't reach for a dark styled
wrapper; the kit sees through and the inner view owns the surface.

---

## 4. Theme tokens in active use

Rustickers currently uses **only the dark theme**. All chrome and
state primitives come from `theme::ActiveTheme` (imported alongside
`use gpui_kit::component::theme::*` in any module that calls
`cx.theme()`).

| Token | When |
| --- | --- |
| `cx.theme().background` | Root manager / selection popup / chip colour backgrounds. |
| `cx.theme().border` | Default 1-px border on cards, inputs, dividers. |
| `cx.theme().muted_foreground` | Secondary copy / placeholder / empty state. |
| `cx.theme().danger` | Error messages, destructive confirmations, expired-run markers. |
| `cx.theme().list_active` | Selected row background. |
| `cx.theme().list_active_border` | Selected row border. |
| `cx.theme().list_hover` | Hovered row background. |
| `cx.theme().transparent` | Inactive transparent surfaces (matches "no decoration"). |

Anything not in this table should come from the kit's built-in
`Theme` palette — do **not** reintroduce `rgb(0x...)` literals for
chrome concerns.

### Verification (run after each UI change)

```bash
rg 'rgba\(|rgb\(0x\b' src/native/        # should return only audited exceptions
rg 'cursor_pointer'       src/native/    # only allowed on real active manipulations
rg 'focus_ring\(false\)'  src/native/    # should return nothing — Kit owns the ring
rg '\.bg\(black\(\)'      src/native/    # should return nothing — Kit theme background
```

The audited exceptions are listed in §7. Outside of those,
CSS-style raw colour literals are a regression.

---

## 5. Component patterns

### Buttons

Always go through Kit variants. The migration landed these in every
chrome / footer / dialog spot:

```rust
use crate::gpui_component::button::{Button, ButtonVariants};  // brings .ghost() / .danger() / .primary() in scope

// Quiet actions: header close, sort chip, footer toolbar, color picker dots
Button::new("close-sticker").ghost().icon(IconName::Close).on_click(…)

// Destructive confirmation — title is "Delete \"<name>\`?" (object + verb), body explains consequence
Button::new("confirm-delete").danger().label("Delete").on_click(…)

// Primary progress (rare in this app)
Button::new("commit").primary().label("Save").on_click(…)

// Segmented selector with explicit selected state (file/audio play-mode toggle)
Button::new("play-mode").selected(self.play_mode == PlayMode::Loop).label("Loop").on_click(…)  // requires Selectable trait
```

`Selectable` is a trait — the file that calls `.selected(...)` must
`use crate::gpui_component::button::Selectable;` (or
`use gpui_kit::component::button::Selectable;`), not just `ButtonVariants`.
A handful of those imports were missed if you ever pull the bridge.

**Don't** manually compose transparent-on-transparent buttons:

```rust
// Anti-pattern — what the migration removed everywhere
Button::new("…")
   .bg(transparent_black().alpha(0.0))
   .border_0()
   .cursor_pointer()
```

Compose with `.ghost()` instead.

### Lists

Long lists go through Kit's `uniform_list`. Both follow the same shape:

```rust
uniform_list("manager-cards", stickers.len(), cx.processor(|this, range, _w, _cx| {
    range.filter_map(|ix| this.stickers.get(ix).cloned())
        .map(StickerCard::render)
        .collect::<Vec<_>>()
}))
```

The clipboard chooser (`src/native/windows/clipboard.rs`) already
follows this pattern; copy it as your scaffold and look at §15 for
the **manager itself**, which is the one outstanding list that
still hasn't been virtualised.

### Inputs and Select-style controls

```rust
use crate::gpui_component::input::{Input, InputState, InputEvent};

let search = cx.new(|cx| InputState::new(cx).placeholder("Search stickers"));
cx.subscribe(&search, |this, _, ev: &InputEvent, _cx| {
    if let InputEvent::Change = ev { this.query(search.read().value().to_string()); }
})
.input(...)
```

Don't paint the input background manually; let the Kit Input own the
focus ring and surface. Adding `.focus_ring(false)` or `.bg(rgba(0x00000000))`
breaks keyboard a11y and was removed everywhere in the migration.

### Dialogs

```rust
use crate::gpui_component::dialog::{Dialog, DialogButtonProps};

Dialog::new("delete-confirm")
    .title(format!("Delete \"{}\"?", title))
    .child(SharedString::from("This removes the sticker and its current contents. You can paste it from the manager afterwards."))
    .action_props(…)
    .button_props(DialogButtonProps::default().ok_label("Delete").kind(ButtonKind::Danger))
    .on_ok(|ok, w, cx| { … })
    .open()
```

The `Delete "<title>?"` shape is the project's reference: object +
verb, no guessing. Use `ButtonKind::Danger` for destructive, never
`.warning()`.

---

## 6. The `StickerColor` palette split

Domain rule: **the model layer holds the serialised identity; the
UI layer holds the actual colour tokens.** Lines stay clean if new
features honor that split.

### Code shape

```rust
// src/model/sticker.rs
pub enum StickerColor { … }

impl StickerColor {
    pub const ALL: [Self; 5] = [Self::Beige, Self::Linen, Self::Gray, …];

    pub fn bg(&self) -> Rgba     { crate::native::theme::sticker_background(*self) }
    pub fn swatch(&self) -> Rgba { crate::native::theme::sticker_swatch(*self) }
}

// src/native/theme.rs
use crate::gpui_component::Rgba;

pub fn sticker_background(c: StickerColor) -> Rgba { match c { … } }
pub fn sticker_swatch    (c: StickerColor) -> Rgba { match c { … } }
```

The UI module re-exports `Rgba` so non-UI crate gates can refer to
it without pulling in `gpui` directly. **Do not move the palette into
`model/`.** Doing so would couple storage / CLI to the gpui
presentation type and would invalidate the regression contract.

### Regression contract

`tests/sticker_palette.rs` pins three properties:

1. Every swatch is bright; every background is dark muted.
2. Every variant in `StickerColor::ALL` produces a unique background
   and a unique swatch.
3. **`StickerColor::bg() == native::theme::sticker_background(...)`**
   and the same for `swatch()`. A drift between model and UI is a
   broken contract.

Run after any palette change:

```bash
cargo test --locked --features cli --test sticker_palette
```

### Future palette promotion (planned)

When `Theme::change(ThemeMode::Light, …)` is wired in, the
five-value `StickerColor` palette should also become a theme
extension (light-mode sticker surfaces) rather than a fixed dark
constant. The current split (UI-owned but constant-valued) is the
intermediate step.

---

## 7. Audited exceptions (do **not** "fix")

These raw colour sites stay by design. If a future contributor
re-writes one of them, they will break a sibling feature they're
not asked about.

| File | Symbol | Why it's allowed |
| --- | --- | --- |
| `stickers/timer.rs` | `green_500()` progress fill | Semantic success state (active vs. expired run). It's a *value marker*, not chrome. |
| `stickers/paint.rs` | 4× `rgba(…)` stroke fills | Data-domain colours: these are the user-picked drawing colours persisted with the paint sticker. Stroke colour is payload, not chrome. |
| `model/sticker.rs` (historically) | raw `rgb(0x…)` | **Now moved**, not eliminated. Reintroducing them here would re-couple the model to the UI palette. |
| `native/theme.rs` | the entire file | This file *defines* the user surface palette. Its existence is what `#5` and the regression contract rely on. |

If you genuinely need to change one of these, open a discussion —
the cached test baseline ensures the regression is visible before
merge.

---

## 8. Spawn / cancel / poll rules

The application's render paths are *expected* to spawn async work
in a few places (timer ticking, markdown autosave, clipboard
ingest). GPUI tolerates this as long as the rule below holds.

### Rule

> Any `cx.spawn` issued inside `Render::render` must be **guarded**
> by an idempotency flag **and** cancelled when the controlling
> `Entity` is dropped or the window closes.

The pattern that satisfies both:

```rust
fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    if !self.loop_running {
        self.loop_running = true;
        let weak = cx.entity().downgrade();
        let task = cx.spawn_in(window, async move |entity, cx| {
            'outer: loop {
                if weak.upgrade().is_none() { break; }
                …
                Timer::after(Duration::from_millis(200)).await;
            }
            if let Some(entity) = weak.upgrade() {
                entity.update(cx, |this, _| this.loop_running = false);
            }
        });
        self.cancel_token = Some(task);
    }
    …
}
```

For window-bound work, prefer `cx.spawn_in(window, …)` so the task
is naturally invalidated when the window tears down. For Entity-only
work, capture a `WeakEntity` plus the entity's own `cx.on_window_close`
hook to keep cancellation correct.

### Polling loops

The clipboard chooser polls capture status at 1 Hz. That loop
should be reworked into an event subscription on the watcher once
the watcher exposes a push API; until then it's tolerated because
the cost is bounded and the watcher debounces file events. Don't
clone this loop shape into other surfaces — push > pull.

---

## 9. Listener / paint patterns

### Use listeners, not raw mouse callbacks

`cx.listener` is the standard closure shape; Kit treats it as the
identity of "which row behaves the same way". Pulling the closure
out of `render` is the next refactor for shared row shapes:

```rust
fn row_listener<E, F>(entity: &Entity<E>, id: i64, handler: F) -> impl Fn(&E, &MouseEvent, &mut Window, &mut App)
where F: Fn(&mut E, i64, &mut Window, &mut App) + 'static { … }
```

Apply at any site that currently rebuilds the same listener 5+
times during a render — manager list, clipboard rows, sticker
chrome are all candidates.

### Per-paint rules

- Inputs/buttons/textareas don't need explicit `.bg(…)`.
- A row's hover state goes through `cx.theme().list_hover`, not
  a per-element `opacity()`.
- The default cursor is the right default. Only override with
  `.cursor_grab()` / `.cursor_pointer()` when the manipulation is
  visible: drag-handle title bars and clickable labels are the
  two allowed cases.
- Cloning entities is the cheap case; cloning strings is the
  expensive case. Prefer `SharedString` (and `render once, cache
  by hash` patterns) over `String::clone` in render paths.

---

## 10. Privacy invariants (do not regress)

These are checked by the 18-test clipboard_storage suite + the
placement integration test (`tests/clipboard_storage.rs`). Anything
you change in clipboard / vault / editor surface flows must still
pass:

```
$ cargo test --locked --features cli --test clipboard_storage
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

What the suite enforces:

- **Redacted previews on protected rows** — protected thumbnails
  never expose plaintext in any code path that surfaces through
  the chooser, vault dialog, or editor body.
- **Vault generation checks** before any rewrap, unprotect, or
  unprotect-via-edit cycle. The vault refuses to operate on a
  generation that doesn't exist.
- **Atomic generation rewrites** — rewrap deletes the old envelope
  in the same transaction it writes the new one, even if those
  two events would naturally be a `BEGIN/COMMIT` pair.
- **Owned-clipboard clearing** — when the user's edit cancels or
  the clipboard gets auto-cleared, the system clipboard is wiped,
  not just the in-memory model.
- **Stale-preview avoidance** — the chooser never re-renders a row
  whose backing data has already been replaced.

When adding a new clipboard feature, add a test that fails *before*
your change and passes *after*. If you can't write the test, the
feature probably crosses a privacy boundary and shouldn't ship.

---

## 11. Native handle parity

Rustickers is a desktop app; kit-level code must not regress the
low-level capabilities that make the app feel native. Treat these
as testing invariants checked at integration time:

- **Transparent sticker corners** — `WindowBackground::Transparent`
  on sticker windows; the inner view paints the colour, the kit
  renders alpha holes.
- **Per-monitor DPI** — `clipboard_ux_smoke` and the sticker view
  scaling must keep producing identical scaled pixel output on
  high-DPI screens. Quick validation: open with
  `rustickers --flash`.
- **Single-instance IPC** — `ipc::SingleInstance::acquire("rustickers")`
  in `src/rustickers.rs`; the secondary process must
  `interprocess::connect_and_signal(name)` to the primary and exit.
  Probe + ack must close the corpse-socket race on macOS.
- **Global hotkeys** — registered via the
  `cx.on_action(Command::new)` pattern during boot. Don't rewrite
  a window to register its own hotkey — keep them in the boot path
  so the chooser picks up the same hotkey contract.
- **WebView lifecycle** — `wry::WebViewBuilder::new_as_child(window)`
  in `native/components/webview.rs`. Don't break the `?` upstream
  flow; if you have to handle a build error, surface it as a
  fallback preview, never as a panic.
- **Paste-destination restoration** — Rustickers distinguishes the
  source window from the destination window before paste. Don't
  lose this when refactoring the chooser's `MouseUpEvent`.
- **Color picker takeover** — `.bg(black().opacity(0.9))` was the
  forced-brown theme bg; remove only after confirming both the
  dark manager surface and the sticker surface paint correctly
  under Kit default theme.

---

## 12. Tests baseline

```
cargo test --locked --features cli
    188 lib + 18 storage + 1 placement + 3 palette = 210 passed
                                                    +  1 ignored (interactive-terminal smoke)
```

Visual UI integration tests via `#[gpui_kit::test]` are **deferred**:

- `gpui-kit`'s `test-support` feature pulls `gpui_web` (wasm) with
  `unicode-properties = ^0.1.3` vs. the closure-locked `0.1.4`. The
  conflict is real and not a Registry failure; it's a kit-side
  preview-build miss.
- Until it's resolved, palette contracts and clipboard storage
  tests cover the migration's public boundary. Visual integration
  coverage of the editor save/cancel, password dialog, vault
  authorize, chooser selection, and paint swatch flows will land
  when the upstream `gpui_web` re-bumps.

When adding tests, prefer:

1. **Storage tests** for any privacy behaviour change.
2. **Palette tests** for any new `StickerColor` variant or any
   change to the UI palette.
3. **UX smoke example** (`examples/clipboard_ux_smoke.rs`) for
   isolated visual behaviour that can't be unit-tested. Run via
   `cargo run --features ui --example clipboard_ux_smoke`.

---

## 13. CI invariants

`.github/workflows/ci.yml` runs on every pull request across
**windows-latest**, **ubuntu-latest** (with webkit+xorg apt deps),
and **macos-14**. The gates, in order:

```bash
cargo fmt          --all -- --check
cargo check        --locked --bin rustickers
cargo check        --locked --no-default-features --features storage
cargo check        --locked --no-default-features --features cli --bin rusticker
cargo check        --locked --features ui
cargo test         --locked --no-default-features --features cli
cargo test         --locked --features cli
cargo build        --locked --features ui --example clipboard_ux_smoke
# Then: a regex-equality check that there's exactly one gpui-pre family in `cargo tree`.
```

If a CI run fails, it's almost always one of:

- **A clippy change in a kit dependency** — bump the kit pin in
  `Cargo.toml` and run `cargo update -p gpui-kit` to refresh the
  closure. Re-run the whole gate sequence.
- **A duplicate `gpui-pre` family** — open `Cargo.lock`, search for
  `[[package]] name = "gpui-pre"`; only one version is allowed.
- **A CRLF/LF formatting churn** — see §14 for the autocrlf
  workaround.

---

## 14. Environment quirks (host-specific)

These are verified quirks on the development host. They'll save
you a few hours if you reproduce them.

### `cargo fmt` resorting imports

After every rename, run `cargo fmt`. Kit's bare-prefix matches
want `crate::gpui_component::*` to come first in its own group;
double-check before committing.

### `USERPROFILE` colon stripping inside `oly start`

The `oly` bash strips the `:` from `USERPROFILE`, turning
`C:\Users\binwenw` into `C:Usersbinwenw`. Cargo's default
`CARGO_HOME = $USERPROFILE/.cargo` then resolves to a wrong
drive-relative location; **builds fail with cryptic "couldn't read
CARGO_HOME" errors**.

Mitigation: run cargo from an interactive bash, or export
`CARGO_HOME=C:/Users/<name>/.cargo` explicitly at the start of the
session.

### Windows artifact lock

`rustickers.exe` is locked by the live test process. A rebuild
will fail with "Access is denied" while the app is running. Recovery:

```bash
# 1. Find the offending PID
tasklist | findstr rustickers
# 2. Either close it cleanly, or use an isolated target directory for the rebuild
CARGO_TARGET_DIR=…target-rebuild cargo build --features ui
```

### `/tmp` paths

Don't pass `/tmp/...` to the `read` tool on this host — it doesn't
resolve. Use absolute Windows paths
(`C:/Users/<name>/AppData/Local/Temp/...`).

### Line endings (cosmetic)

Global `core.autocrlf=true` produces LF→CRLF warnings on every
`git add` on Linux-style files. This is your personal config; the
repo has no opinion. Add a `.gitattributes` if you'd rather normalise.

---

## 15. Roadmap (post-kit-migration)

The PR review identified the follow-ups in priority order. Tackle
in this order unless your feature intersects.

| Priority | Item | Why it tops the list |
| --- | --- | --- |
| **P0** | **§1.1 — virtualise the manager sticker list** with Kit `List`/`uniform_list` (`STICKER_LOAD_LIMIT = 10000` is rendered eagerly today; per-row `cx.listener` closures are reallocated every render). | Single largest performance and accessibility win. |
| **P0** | **§4.1 — manager keyboard path + context menu**: `Tab` to enter the list, `↑/↓` to navigate, `Enter` to open, `F2` to edit, `Del` to remove; right-click context menu. | Falls out for free after `List` migration. |
| **P1** | **§2.1 — bind timer / markdown autosave loops to the window lifecycle** via `cx.spawn_in(window, …)` and `cx.on_window_close`. | Reliability; the guard already exists, the cancellation doesn't. |
| **P1** | **§4.2, §4.5 — footer hint surface and status banner alignment** with Kit `Alert::error(...).small()`. | Cheap UX wins, no behavior change. |
| **P1** | **§2.4 — IPC single-instance hardening** (ack probe, `'ctrl-c' cleanup`, explicit permission-denied branch). | Recurring ticket fix on macOS. |
| **P2** | **§5.1 / §5.2 — listener-as-data + `CardSurface` refactor**. | Cleanup with secondary performance benefits. |
| **P2** | **§3 reliability — SQL contention diagnostics; install a panic hook** that writes `crash_log.txt`. | Polish; profile what's actually slow first. |
| **P2** | **§4.10 — system tray icon** for "collection paused" status. | Out-of-scope fluff for the migration, but the single biggest continuous-discovery win. |

There are also longer-horizon items:

- Resolve `gpui-kit`'s `test-support` so `#[gpui_kit::test]` UI
  integration tests can run in CI.
- Promote the StickerColor palette to a true theme extension so
  light mode brings the surface tints with it.
- Replace `screenshots/demo1.png` after a native visual review.

---

## 16. Reference skill catalog

The local skill snapshots document the kit's own conventions. They
were the authoritative reference during the migration and stay
current with the v0.7.1 source.

```text
.agents/skills/gpui-kit/
  SKILL.md
  references/
    coding-guides.md        ← normative Coding Guide
    conventions.md          ← module / naming posture
    recipes.md              ← state, lifecycle, async + cancellation recipes
    gpui/                   ← per-topic references (entity, async, focus, …)
.pi/skills/gpui-kit-design-guides/
  SKILL.md
  references/
    design-guides.md        ← normative Design Guide
skills-lock.json            ← source URL + computed hash per skill
```

The important takeaways from the migration's audit, milestone log,
post-migration review, and clipboard UX guides have been distilled
into the sections above. When you change a §1–§15 rule, mirror it
back into the matching local skill reference under
`.agents/skills/gpui-kit/reference/` or
`.pi/skills/gpui-kit-design-guides/references/` so future snapshots
of those skills stay current.

---

## 17. Quick checklist for new surface work

Before you open a PR that touches the desktop UI, walk this list:

- [ ] Used `cx.theme().*`, not raw `rgb(0x…)` (with the §7 exceptions).
- [ ] Used Kit button variants (`Button::ghost()` / `.danger()` / `.selected(...)`),
      not manual `bg(transparent_black)` + `border_0()` compositions.
- [ ] New long lists go through `uniform_list`, not `v_flex().children(…)`.
- [ ] Spawned tasks from render are guarded + cancellation-bound to the
      controlling `WeakEntity` and the window's close hook.
- [ ] Any clipboard / vault change re-runs `cargo test --features cli
      --test clipboard_storage` — **210 passed, 0 failed** must hold.
- [ ] Any palette change re-runs `cargo test --features cli --test
      sticker_palette` and (if you add a variant) extends the model↔UI
      regression contract.
- [ ] `cargo fmt` is clean; `cargo check --locked --features ui` is
      clean; `cargo clippy --locked --features ui` is clean except the
      one documented exception at `src/native/components/stickers/file/mod.rs:588`
      (replace with `!matches!(...)` before your next round if you touch file/mod.rs).
- [ ] If your change affects a kit-exposed public surface or imports
      shape: think about whether to start removing the namespace bridges
      (§1). They are now vestigial from the module-level view; they
      will eventually come out.
- [ ] If your change supersedes a §2–§15 rule, mirror it back into
      the matching entry in `.agents/skills/gpui-kit/reference/` or
      `.pi/skills/gpui-kit-design-guides/references/` (see §16).

If all boxes tick, ship it.
