# Rustickers frontend handbook

> First stop for any change that touches the desktop UI, theme,
> sticker rendering, clipboard surface, or window code. `README.md`
> tells you what the app does and how to run it; this file tells you
> how it is built.

---

## 1. Stack and dependency closure

- **UI framework**: `gpui-kit 0.7.1` from crates.io — a single
  facade that re-exports `gpui`, `gpui-component`, `gpui-base`,
  `gpui-assets` and a matching `gpui-wry` adapter. The old Git
  pins (`gpui` / `gpui-platform` / `gpui-component` / `gpui-wry`)
  are gone.
- **Closure invariant**: `gpui-pre 0.3.8` exists **exactly once** in
  `cargo tree`. The same crates can only satisfy that by going
  through the kit's re-exports. Verify any time:

  ```bash
  cargo tree --features ui --edges normal | grep '^name = "gpui-'
  ```

  Two distinct versions means a transitive drift; stop and fix.

- **Bridges in `src/lib.rs`**: two namespace aliases still exist for
  the migration hand-off:

  ```rust
  #[cfg(feature = "ui")]
  extern crate gpui_kit as gpui;
  #[cfg(feature = "ui")]
  pub mod gpui_component { pub use gpui_kit::component::*; }
  #[cfg(feature = "ui")]
  extern crate gpui_wry;
  #[cfg(feature = "ui")]
  extern crate lb_wry as wry;
  ```

  Submodule bare-name resolution only reaches extern crates, so
  every `use gpui_component::…` in a source file was rewritten to
  `use crate::gpui_component::…`. The bridges can come out once
  every module imports `gpui_kit::*` directly. Verify stragglers:

  ```bash
  git grep -nE '^\s*use gpui_component::' src/ tests/ examples/
  ```

---

## 2. Where things live

```
src/
  lib.rs              ← namespace bridges, public re-exports
  rustickers.rs       ← CLI / `SingleInstance` IPC / glue
  model/sticker.rs    ← StickerColor serialised identity (no UI literals)
  native/
    mod.rs            ← `--features ui` boot: gpui::application(), theme, watcher
    theme.rs          ← UI-owned StickerColor palette split (§6)
    hotkey.rs         ← Global hotkey dispatcher
    components/
      webview.rs      ← WRY WebView lifecycle, fallible construction
      stickers/
        markdown.rs   content_lock.rs  timer.rs  paint.rs
        command/      file/             (per-family modules; new ones go here
                                          and `pub mod` is wired in stickers/mod.rs)
    windows/
      main.rs         ← manager (uniform_list, keyboard nav, revision token)
      selection.rs    ← selection popup
      clipboard.rs    ← clipboard chooser (uniform_list, confirm-then-forget)
      clipboard/editor.rs
      sticker/{open.rs,view.rs,mod.rs}
tests/
  clipboard_storage.rs  ← 18 privacy regression tests -- DO NOT regress
  sticker_palette.rs    ← 3 StickerColor regression tests
examples/
  clipboard_ux_smoke.rs ← isolated visual smoke harness
.github/workflows/ci.yml
.cargo/                    (configurations, ignored by git)
```

Adding a new family: drop `mod <family>;` into `src/native/components/stickers/mod.rs`,
mirror the boilerplate of `timer.rs`/`markdown.rs`, and reuse the
shared services from `crate::native::components::stickers::…`
(e.g. `FOCUS_LOSS_RELOCK_DELAY`).

---

## 3. Bootstrap

`src/native/mod.rs` boots the UI under `#[cfg(feature = "ui")]`. Three rules:

1. `gpui::application()` factories the app. Don't reach for the legacy
   `Application::with_platform(gpui_platform::current_platform(false))`.
2. **Don't override the theme.** No `cx.global_mut::<Theme>().background = …`
   calls. The kit-default theme is the surface; light/dark variants
   swap through `Theme::change(…)` when wired.
3. **The kit root owns overlays.** No `.children(Root::render_dialog_layer(w, cx))`
   in main or sticker render. `gpui_kit::component::init(cx)` is called
   once during boot and the plugin model registers dialogs.

Sticker windows need alpha corners -- construct with
`WindowBackgroundAppearance::Transparent`. The inner view owns the colour.

---

## 4. Theme tokens

`cx.theme()` is `gpui_kit::component::theme::ActiveTheme`. The project's
chrome reads only these tokens:

| Token | Use |
| --- | --- |
| `background` | Root manager / window surface |
| `border` | Default 1-px border on cards, inputs, dividers |
| `muted_foreground` | Secondary copy, placeholders, empty states |
| `danger` | Error messages, destructive confirmations, expired-run marker |
| `list_active` | Selected row background |
| `list_active_border` | Selected row border |
| `list_hover` | Hovered row background |
| `transparent` | Inactive transparent surface |

For anything else, use `cx.theme().*` or add the role to the project
theme. No raw `rgb(0x…)` / `rgba(…)` literals for chrome concerns
(see §7 for the four audited exceptions).

```bash
rg 'rgba\(|rgb\(0x\b' src/native/        # audited exceptions only
rg 'cursor_pointer' src/native/         # only allowed on real active manipulations
rg 'focus_ring\(false\)' src/native/    # must return nothing
```

---

## 5. Component patterns

### Buttons

```rust
use crate::gpui_component::button::{Button, ButtonVariants, Selectable};

Button::new("close").ghost().icon(IconName::Close).on_click(…)
Button::new("confirm").danger().label("Forget").on_click(…)   // destructive
Button::new("play").selected(self.mode == Mode::Loop).label("Loop").on_click(…)
```

- `ghost` for chrome (close, sort, footer toolbar, color dots).
- `danger` for irreversible confirmations.
- `selected(...)` for segmented control state -- import `Selectable`,
  not just `ButtonVariants`.
- Title is `Forget "<object>"?`, body names the consequence.
  `ButtonKind::Warning` is not used here.

### Lists

`uniform_list` for any long collection. Both the manager and chooser
use it; the closure captures `cx.entity()` and routes through
`view.update`:

```rust
let view = cx.entity();
uniform_list("manager-stickers", self.stickers.len(),
    move |range, _window, cx| view.update(cx, |this, cx|
        range.into_iter().filter_map(...)  .collect::<Vec<_>>()
    ),
)
.size_full()
.track_scroll(&self.list_scroll);
```

Fixed row height is required by `uniform_list`; in this codebase
`ROW_HEIGHT = 80.0` (manager) and `44 px` (chooser rows).

### Inputs

Don't paint the input background, don't disable the focus ring --
`Input::new(&self.state)` owns the focus ring and the surface.

```rust
let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search stickers"));
Input::new(&self.query).cleanable(true).border_0().tab_index(0).prefix(Icon::new(IconName::Search))
```

### Dialogs

Construct via `window.open_dialog(cx, |dialog, _, _| builder)` -- the
dialog has no `.open()`; the helper opens it.

```rust
window.open_dialog(cx, move |dialog, _, _| {
    dialog
        .title(format!("Forget \"{preview}\"?"))
        .child(div().text_sm().child(body))
        .footer(h_flex()
            .gap_2().justify_end()
            .child(Button::new("cancel").label("Cancel").on_click(|_, w, cx| w.close_dialog(cx)))
            .child(Button::new("forget").label("Forget").danger()
                .on_click(|_, w, cx| { w.close_dialog(cx); /* … spawn delete … */ })))
        .w(px(420.0));
});
```

### Keyboard

Container focus: `cx.focus_handle().tab_index(n).tab_stop(true)` +
`.track_focus(&handle)`. `on_key_down(cx.listener(…));` to read
`KeyDownEvent` directly, with `cx.stop_propagation()` +
`window.prevent_default()` for handled keys.

---

## 6. `StickerColor` palette split

The model holds identity; the UI holds the surface colours. Keep
categories from drifting.

```rust
// src/model/sticker.rs
impl StickerColor {
    pub const ALL: [Self; 5] = [...]
    pub fn bg(&self) -> Rgba     { crate::native::theme::sticker_background(*self) }
    pub fn swatch(&self) -> Rgba { crate::native::theme::sticker_swatch(*self) }
}

// src/native/theme.rs
pub fn sticker_background(c: StickerColor) -> Rgba { match c { … } }
pub fn sticker_swatch    (c: StickerColor) -> Rgba { match c { … } }
```

`tests/sticker_palette.rs` pins three properties:

1. Every swatch is bright; every background is dark muted.
2. `StickerColor::ALL` produces distinct backgrounds and swatches.
3. **`StickerColor::bg() == native::theme::sticker_background(...)`**
   and the same for `swatch()`. A drift here is a regression.

```bash
cargo test --locked --features cli --test sticker_palette
```

---

## 7. Audited exceptions (do not "fix")

| File | Site | Why it stays |
| --- | --- | --- |
| `stickers/timer.rs` | `green_500()` progress fill | Semantic success state (active vs. expired run). |
| `stickers/paint.rs` | 4× stroke `rgba(…)` | Data-domain: user-picked stroke colours persisted with the paint sticker. |
| `native/theme.rs` | the entire file | Defines the surface palette; the test contract depends on it. |

If a feature genuinely needs to change one, open it as a discussion --
the regression contract will catch a wrong intent early.

---

## 8. Spawn / cancel / poll rules

Three categories of async work exist in this app:

| Surface | Owner | Cancellation |
| --- | --- | --- |
| Tick loops (timer, command scheduler) | One `tick_revision: u64` on the controller entity | Loop body compares the captured revision against the entity's; bumps supersede. Loop breaks on `entity.update_in(...).is_err()`. |
| I/O dispatch (clipboard, file IO, vault) | Per-event owner; spawn from `cx.listener` | Same `update_in` error break; db transactions handle their own atomicity. |
| Watcher thread | Dedicated OS thread with its own `AtomicBool` stop signal | Not going through `cx.spawn`; control sits in `WatcherHandle::spawn(self, ...).detach()`. |

The non-negotiables:

- **Do not spawn from inside `Render::render`** -- the entity doesn't
  have a stable revision at that point and a re-render can leak
  another instance. Spawn from `new()` or from a state-transition
  method like `begin_tick_loop(...)`.
- **Don't conflate revision and guard**. The guard answered *some*
  loop is alive. The revision answers *this* loop is still wanted.
- **`cx.spawn_in(window, …)` is not auto-cancellation.** The future
  runs until its next `await` regardless of whether `window` or
  the entity still exists. Cancellation is by `update_in` returning
  `Err` (entity gone) or by a revision tag mismatch (state changed).

```rust
// Revision-tagged tick loop pattern (timer.rs).
let revision = self.tick_loop_revision;
cx.spawn(async move |entity, cx| {
    loop {
        let still_wanted = entity.update(cx, |this, _| {
            this.tick_loop_revision == revision
        }).unwrap_or(false);
        if !still_wanted { break; }
        // … tick body …
    }
}).detach();
```

A new state transition (start / pause / resume / restart) calls
`self.supersede_tick_loop()` which bumps the revision; the old
loop exits on its next iteration without explicit cancellation.

---

## 9. Listener / paint rules

- Buttons / inputs / textareas paint themselves. Don't override
  `.bg(...)` on them.
- Default cursor is the right default. Override with
  `.cursor_grab()` / `.cursor_pointer()` only when the manipulation
  is visible: drag-handle title bars and clickable inner labels.
- Sticker cards keep **fixed `ElementId`s** of
  `("sticker-row", id as usize)` so hover/selected/focused state
  survives re-ordering by the event pump.
- Prefer `SharedString` over `String::clone` in render paths when
  the same string is shown by multiple elements.

---

## 10. Privacy invariants

Verified by the 18-test `clipboard_storage` suite and the placement
integration test. Anything that touches clipboard, vault, or
editor flows must keep passing:

```
cargo test --locked --features cli --test clipboard_storage
→ 18 passed
```

What's enforced:

- **Redacted previews on protected rows** -- no plaintext field
  surfaces through the chooser, vault dialog, or editor body.
- **Vault generation checks** before any rewrap, unprotect, or
  unprotect-via-edit cycle. New envelopes don't overwrite stale
  generations.
- **Atomic generation rewrites** -- rewrap deletes the old
  envelope in the same transaction it writes the new one.
- **Owned-clipboard clearing** -- on action cancel or auto-clear,
  the system clipboard is wiped, not just the in-memory model.
- **Stale-preview avoidance** -- the chooser never re-renders a
  row whose backing data has already been replaced.

Capture filter rules live in `src/native/clipboard/watcher.rs`. The
fail-closed pause on protected history is enforced by the storage
`has_protected_items()` query (single `SELECT EXISTS`); see
`tests/clipboard_storage.rs` for the contract.

---

## 11. Native-handle parity

| Capability | Where it lives |
| --- | --- |
| Transparent sticker corners | `WindowBackgroundAppearance::Transparent` in `sticker/open.rs`. |
| Per-monitor DPI | `--flash` UX smoke + native window handles; verify at integration time, not via `cargo test`. |
| Single-instance IPC | `ipc::SingleInstance::acquire("rustickers")` in `src/rustickers.rs`. The secondary process `connect_and_signal`s the primary and exits. |
| Global hotkeys | `src/native/hotkey.rs` dispatcher; routes through `IpcEvent` rather than per-window registration. |
| WebView lifecycle | `SimpleWebView::try_new` is fallible (`Result<Entity<Self>, String>`); the file preview propagates `Err` into `this.error = Some(...)`. |
| Paste-destination restoration | The chooser captures the focused window id at hotkey time and passes it through `use_selected(...)` so `foreground(destination)` lands on the original app. |
| Confirm-then-forget | `remove_selected` in `clipboard.rs` fans out a `confirm_remove(...)` dialog when the row is protected or retained; ordinary disposable rows delete instantly. |

---

## 12. Tests, gates, and CI

Local baseline on a clean tree:

```
cargo test --locked --features cli
→ 190 lib + 18 storage + 3 palette + 1 placement = 212 passed
                                                       + 1 ignored (interactive)
```

PR CI (`.github/workflows/ci.yml`) runs on `windows-latest`,
`ubuntu-latest` (with webkit + xorg apt deps), and `macos-14`:

```bash
cargo fmt          --all -- --check
cargo check        --locked --bin rustickers
cargo check        --locked --no-default-features --features storage
cargo check        --locked --no-default-features --features cli --bin rusticker
cargo check        --locked --features ui
cargo clippy       --locked --features ui
cargo test         --locked --no-default-features --features cli
cargo test         --locked --features cli
cargo build        --locked --features ui --example clipboard_ux_smoke
```

Plus a metalava-like check that `cargo tree --features ui` contains
exactly one `gpui-pre` family.

UI integration tests via `#[gpui_kit::test]` are deferred:
`gpui-kit`'s `test-support` depends on `gpui_web` (`wasm`) whose
`unicode-properties = ^0.1.3` conflicts with the closure-locked
`^0.1.4`. Visual flows are covered by `clipboard_ux_smoke.rs`
until upstream resolves the gap.

---

## 13. Status of the post-kit work

The plan that came out of the post-kit code review had six phases.
Currently:

| Phase | Status | Commit |
| --- | --- | --- |
| P0: Timer task ownership | Done | `9aeb6a9` |
| P0: Manager async correctness (latest-request policy, supersede delete, owner `open_sticker`) | Done | `7cee743` |
| P1: Manager virtualization + keyboard nav (uniform_list, Tab/Enter/Delete) | Done | `51d6d3b` |
| P1: Recoverable WebView (`SimpleWebView::try_new -> Result<…>`) | Done | `f6cd76e` |
| P1: Clipboard read efficiency (existence query, content-free `WatcherSignal`, 5 s reconciliation) | Done | `eb1b5ec` |
| P2: Confirm-then-forget for protected / retained clipboard rows | Done | `5521df4` |

Open follow-ups: UI integration tests once `gpui-kit`'s
`test-support` re-bumps; status-banner alignment with Kit elevation;
sticker palette → theme extension when light theming lands;
`screenshots/demo1.png` replacement after a native visual review.

---

## 14. Reference skill catalog

```text
.agents/skills/gpui-kit/                SKILL.md + references/
                                       (coding-guides, conventions, recipes, gpui/**)
.pi/skills/gpui-kit-design-guides/      SKILL.md + references/design-guides.md
skills-lock.json                       source URL + computed hash per skill
docs/plans/gpui-kit-upgrade{,-goal}.md  (deleted; the takeaways live in §1–§13)
docs/plans/clipboard-{coding,design}-guides.md  (deleted; the
                                                  takeaways live in §4–§10)
```

The local skill snapshots are the authoritative reference for kit
conventions during a migration and stay current with the v0.7.1
source. Read them for full design/coding details; this file
distils the project-specific consequences.
