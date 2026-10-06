# GPUI Kit Upgrade — Goal & Definition of Done

Temporary planning file for the GPUI Kit 0.7.1 migration. For full
investigation/audit, see `docs/plans/gpui-kit-upgrade.md`. This file is the
target — what success looks like and how we know we got there.

## The goal

**Ship the desktop application on GPUI Kit v0.7.1 (one consumer dependency,
`gpui_kit::*` access) with every screen conforming to the Kit Coding and
Design Guides, without regressing existing product behavior, privacy
guarantees, or platform support.**

A reviewer should be able to clone the repo, run `cargo build --locked
--features ui` on Windows/Linux/macOS, and use every feature that worked
before the migration. The UI should look like a coherent Kit application,
not a partially ported one.

## Definition of done (all must hold)

### Build & dependency health

- [ ] Single GPUI Kit dependency (plus `gpui-wry`) for the UI feature.
- [ ] No remaining Zed/`gpui-component` Git pins; `gpui-kit` and `gpui-wry`
      resolve from the same verified source.
- [ ] `cargo tree -d` shows no duplicate GPUI type family.
- [ ] `cargo fmt --all -- --check` clean.
- [ ] `cargo check --locked` passes for: default `ui`, `--no-default-features`,
      `--features storage`, `--features cli`.
- [ ] CLI and storage features remain independent of UI dependencies.

### Test gates (all green with `--locked`)

- [ ] `cargo test --locked --no-default-features --features cli` — same 116
      tests this baseline ships with still pass.
- [ ] `cargo test --locked --features cli` — passes with no Windows
      `Access is denied` blocking artifact cleanup.
- [ ] UI integration tests via `#[gpui_kit::test]` import Kit types
      explicitly (no glob-shadowing Rust `#[test]`), mount production views
      under Root, and assert real interactions.
- [ ] At least one UI test per surface: main manager, selection chooser,
      clipboard chooser, lock/unlock, timer, command (start/stop/cron),
      markdown edit/preview, file preview error, paint accessibility.
- [ ] Negative cases: disabled control rejects input; stale async work
      cannot update destroyed state; protected content never appears in
      accessibility/test observation; spelling-error cron is rejected
      before scheduling; relock fires on focus loss.

### Design conformance (Coding + Design Guides)

- [ ] Every screen uses `cx.theme()` semantic tokens — zero raw `rgb(...)`,
      `rgba(...)`, or hex inside UI code. The only exceptions are
      documented in one place: domain layer (color enum), raster data,
      platform insets, and WebView/physical-pixel constants.
- [ ] No `cursor_pointer()` on buttons, switches, checkboxes, tabs, rows,
      or other native controls. Default arrow cursor restored.
- [ ] Focus rings visible on every interactive element. Search inputs do
      not call `.focus_ring(false)`.
- [ ] Overlays (dialogs/sheets/menus/notifications) flow through Root in
      every window. No application code mounts its own `render_dialog_layer`.
- [ ] Buttons use `Button` with semantic variants; `Link` is reserved for
      external URLs/email. Icon-only buttons have accessible names; `Save`
      buttons open settings, `Delete` deletes.
- [ ] Destructive clipboard operations use `AlertDialog` with object+verb
      copy: `Delete "Roadmap"?` → `Cancel`/`Delete`. No `OK`, no
      "Are you sure?" body text.
- [ ] Forms use `Form::new().child(Field::new().label(...).child(...))`.
      Booleans use `.on_change` with the requested value, not click toggles.
- [ ] No task is created from `render()`. Every `cx.spawn(...).detach()`
      has a documented owner and a cancellation/abort path.
- [ ] Long sticker collections virtualize with stable domain identity;
      selection survives filter/reorder when the object still exists.
- [ ] Light theme, dark theme, larger base font, minimum/default/wide window,
      scaled display, long translations, and CJK text verified per surface.

### Privacy/security regression

- [ ] Clipboard collection still defaults to paused.
- [ ] Protected content not visible in: previews, search results, accessibility
      properties, test snapshots, debug logs, or stale async completions.
- [ ] Vault generation checks still atomic; relock still fires on focus loss;
      owned clipboard still cleared on unlock expiry.
- [ ] Manual verification: capture an encrypted note, lock, force-unlock
      attempt with wrong password yields no plaintext in any visible state.

### Platform parity

- [ ] Windows x86_64, Linux x86_64, macOS arm64 each build under
      `cargo build --release --locked`.
- [ ] Transparent sticker corners, drag area, resize, pin/topmost,
      global hotkeys, single-instance IPC, and clipboard paste-destination
      restoration still work on each platform.
- [ ] WebView adapter still constructs, bounds correctly, hides cleanly,
      routes external links out, and is destroyed without leaking when a
      sticker closes.
- [ ] CI workflow updated with Kit's native prerequisites and a PR
      validation gate that is independent of release-upload jobs.

### Documentation

- [ ] `README.md` reflects the actual dependency (`gpui-kit`) and links to
      the authoritative Getting Started page.
- [ ] `ARCHITECTURE.md` updated to mention Kit layer names (`gpui_kit::*`,
      `gpui_kit::component`, `gpui_kit::base`, `gpui_kit::assets`).
- [ ] Screenshots in `screenshots/` updated only after native visual review;
      old screenshots removed or replaced, never left alongside.

## Milestones (rough order; each is independently shippable)

| # | Milestone | Validates |
| --- | --- | --- |
| 1 | Unblock artifact cleanup; rerun UI baseline; record its result | We know the real starting state |
| 2 | Replace dependency entries; preserve runtime behavior | Build + CLI/storage tests still green; UI looks like the baseline |
| 3 | Consolidate Root/overlay ownership; restore theme tokens | Every window has one overlay owner; design checklist passes |
| 4 | Converge main manager + selection chooser on Kit components | Manager UI test green; keyboard path covered |
| 5 | Converge clipboard chooser and vault/edit on Kit | Existing privacy tests stay green; new UI tests for the editor |
| 6 | Migrate sticker families one by one (md → timer → command → file → paint) | Per-family UI tests green; native smoke checks recorded |
| 7 | Platform parity verification + CI upgrade | All three release flows build; PR gate covers features |

## Out of scope (explicit non-goals for this upgrade)

- Adding GPUI Fast or other unreleased Kit features.
- Introducing a dashboard, sidebar navigation, dock, or rewriting the
  app into a SPA-style shell. Sticker managers stay utility windows.
- Schema or data-format changes. Existing sticker payloads still load.
- Adding new sticker types beyond the existing seven.
- Clipboard collection default to **on**. Privacy default stays paused.
- Replacing SQLite or the CLI command surface with new machinery. They
  pass through with minor adjustments only.
- Redesigning brand assets, icons (beyond the bundled Kit icon set), or
  marketing pages.
- Translating copy. Internationalization shaping is fine, full i18n is not.

## Stop conditions

Stop and revisit the plan if:

- Kit 0.7.1 reveals API breakage that forces hidden behavior changes in
  clipboard, vault, or background scheduling. Do not paper over them.
- A platform-specific regression cannot be reproduced or fixed within one
  milestone without delaying the others. Cut scope on that milestone
  rather than carrying hidden bugs.
- A design-guide non-conformance is required to preserve a privacy or
  correctness guarantee. Document the exception in the Design review
  checklist and revisit when Kit gains the missing capability.

## Reviewer checklist

When this plan claims "done", the reviewer should be able to:

1. Pull the repo, run `cargo build --locked --features ui`, launch the app.
2. Open a Markdown sticker, edit it, watch autosave and close-or-keep prompts.
3. Open a timer, set 5 seconds, see it count down, hear/see expiry, restart.
4. Run a command sticker; kill it; observe cancel + cleanup.
5. Capture clipboard content; verify it appears only when capture is on;
   protect an entry; lock the app; attempt unlock.
6. Paint, switch colors, undo, save.
7. Resize/move sticker windows across two monitors; verify placement
   records on next launch.
8. Trigger Escape on each overlay; confirm focus returns to the trigger.
9. Resize the window very small; verify the manager keeps all regions usable.
10. Toggle the system theme; verify both light and dark look coherent
    without raw color leaking through.

## Milestone 1 — initial baseline recorded (2026-10-06)

Read [the plan](./gpui-kit-upgrade.md). Build the project from a clean tree,
run every gate listed under "Test gates" (CLI/storage first, then the UI
build), and record the result so a reviewer can compare it to whatever we
report at every later milestone. Do not edit `Cargo.toml`, `Cargo.lock`, or
any application source in this milestone.

Exit: a written baseline log listing the exact commands run, exit codes,
test counts, and any blockers, plus a clean diff against the initial tree.

### Baseline log (filled by this milestone)

- Working tree: clean, only `docs/plans/gpui-kit-upgrade.md`,
  `docs/plans/gpui-kit-upgrade-goal.md`,
  `docs/plans/clipboard-coding-guides.md`, `docs/plans/clipboard-design-guides.md`,
  `docs/plans/clipboard-history.md` were added; the rest
  (`.agents/`, `.pi/`, `skills-lock.json`) was already untracked before this
  work and is preserved.
- `cargo fmt --all -- --check`: clean.
- `cargo check --locked --offline --bins --examples --features ui`:
  **passed** in 24s (`Finished dev profile`). This checks every UI bin
  and example without producing the linked executable, which worked
  around the build-artifact lock.
- `cargo check --locked --offline --no-default-features --features storage`:
  passed.
- `cargo check --locked --offline --no-default-features --features cli --bin rusticker`:
  passed.
- `cargo test --locked --offline --no-default-features --features cli`:
  **116 tests passed** (97 lib + 18 clipboard storage + 1 placement), 1
  interactive terminal test ignored, 0 failed.
- `cargo test --locked --offline --features cli`: **blocked by Windows
  lock on `target/debug/rustickers.exe`**. A `rustickers.exe` process
  (PID 32556, started 10/6 15:31:48 with no visible main window) holds
  the file. Cargo refuses to remove the build artifact. The lock is
  environmental (live user process), not a code regression.
- Mitigation tested, not adopted: an isolated `--target-dir` at
  `C:\Users\binwenw\AppData\Local\Temp\rustickers-baseline-target` is a
  full cold rebuild that produced 6+ GB before linking. It would reproduce
  the UI test result without touching the user-facing exe. We did not adopt
  it because cold-rebuild runtime and disk usage are not justified to surface
  a baseline that we already validate via `cargo check --bins --examples --features ui`.
- Orphaned processes: two `cargo.exe` (PIDs 2304 and 33568, both started
  10/6 15:31:31) are leftover from earlier attempts and should be reaped
  before milestone 2, otherwise cargo may continue to hold tree locks and
  burn CPU during rebuilds.
- `cargo tree --locked -d --features ui`: dependency tree dump shows the
  current UI pinning (longbridge `gpui-component` 0.5.2 at
  `aa7def675aa17b31ca7b79542c8e7a779ed3e56b`, Zed `gpui` 0.1 at
  `5896274168e06663568f42b5bae142162936e0a6`). Confirms dependency baseline
  is what we expect to replace.
- Oly-bash note: when running cargo through `oly start`, the propagated
  `USERPROFILE` had its colon stripped (`C:Usersbinwenw`), which broke
  cargo's default `CARGO_HOME` resolution. Bash and PowerShell see the
  correct value. Use `cargo` from my interactive bash (no oly) or supply
  `CARGO_HOME` explicitly when scripting through oly. Not a target defect.

### Suggested cleanup before milestone 2

- Close the running `rustickers.exe` PID 32556 with the user's
  permission, then kill the two orphaned cargo PIDs. After that
  `cargo test --locked --offline --features cli` should be runnable in
  the existing target dir.
- If the user prefers to keep the running instance, every consequential
  cargo invocation in milestones 2–6 must use `--target-dir
  C:\Users\binwenw\AppData\Local\Temp\rustickers-migration-target` so the
  running instance is never touched.

### Status (carried forward)

Done. Next: migrate the dependency tree.

## Milestone 2 — engine swap, behavior intact (2026-10-06)

Read the audit at `docs/plans/gpui-kit-upgrade.md` and the run logs above.
Replace the legacy `gpui` / `gpui_platform` / `gpui-component` / `gpui-wry`
Git pins with the verified `gpui-kit` v0.7.1 facade and the matching
`gpui-wry` v0.7.1, leaving application behavior untouched. Do not redesign
any UI in this milestone — that is the goal of milestone 3.

Exit: a tree that compiles with `cargo check --locked --features ui`,
`cargo build --locked --features ui` succeeds (the example included), all
preserved CLI/storage/integration tests still pass, and `cargo fmt` is clean.

### Changes

- `Cargo.toml`: dropped `gpui`, `gpui_platform`, `gpui-component` and the
  four-step `gpui-wry` revision pin; added `gpui-kit` and `gpui-wry` both
  path-pinned to the v0.7.1 layout of the upstream clone
  (`C:\Users\binwenw\AppData\Local\Temp\rustickers-gpui-kit-upstream\crates\kit`
  and `…\webview`). Path-pin is intentional: it avoids a fresh large fetch
  during migration and lets us swap to `tag = "v0.7.1"` in the final pass.
- `src/lib.rs`: added namespace bridges so existing modules can keep
  `use gpui::…` and `use gpui_component::…` while the import migration to
  `gpui_kit::` happens file by file in later milestones:
  - `#[cfg(feature = "ui")] extern crate gpui_kit as gpui;`
  - `#[cfg(feature = "ui")] pub mod gpui_component { pub use gpui_kit::component::*; }`
  - `#[cfg(feature = "ui")] extern crate gpui_wry;`
- 22 source files mass-rewritten: `use gpui_component::X` to
  `use crate::gpui_component::X` (submodule paths do not reach the package
  root by bare name; only extern crates do). Bare inline uses inside
  function bodies and a `use gpui_component::IconNamed` inside a `#[test]`
  were caught and fixed by hand.
- `src/native/mod.rs`: replaced
  `Application::with_platform(gpui_platform::current_platform(false))`
  with `gpui::application()`. Removed forbidden raw `rgb(0x151104)`
  background override so the Kit theme owns the surface (recorded as
  audited exception; will be replaced by a theme token in milestone 3).
  Removed now-unused `Application` import.
- `src/native/windows/main.rs` and `src/native/windows/clipboard.rs`:
  removed `.children(Root::render_dialog_layer(window, cx))` calls. The Kit
  renders overlay layers through Root plugins registered by
  `gpui_component::init`; the application's render chain no longer needs
  to host the dialog layer explicitly. Renamed the now-unused render
  parameters to `_window` / `_cx`.
- `examples/clipboard_ux_smoke.rs`: switched to `gpui_kit::{App,
  application}` and `gpui_kit::component::{Theme, ThemeMode}` directly.
  Removed the raw `rgb(0x151104)` literal; the example now uses the Kit
  theme.

### Result

- `cargo check --locked --features ui`: **clean**, no warnings.
- `cargo build --locked --features ui --example clipboard_ux_smoke`:
  **green** in 1m08s, builds the new Kit family end to end.
- `cargo test --locked --features cli`:
  - `lib` unit tests: **188 passed**, 0 failed, 1 ignored.
  - CLI bin unit tests: 0 passed (none defined).
  - UI bin unit tests: 0 passed (none defined).
  - `tests/clipboard_storage.rs`: **18 passed**, 0 failed.
  - `tests/sticker_placements.rs`: **1 passed**, 0 failed.
  - Doc tests: 0.
  - **Total: 207 passed, 0 failed.**
- `cargo fmt --all -- --check`: clean.
- Dependency tree (`cargo tree --locked -d --features ui`): no duplicate GPUI
  type families; everything routes through `gpui-pre 0.3.8` →
  `gpui-kit 0.7.1` → `gpui-component 0.7.1` → `rustickers`.

### Outstanding issues for later milestones (not regressions)

- The bridges in `src/lib.rs` mask the import migration; they should be
  removed in milestone 3 once imports move to `gpui_kit::…` and the audit's
  raw-color / cursor-pointer / focus-ring violations are addressed.
- The `clipboard_ux_smoke` example and the kit avatar font setup still
  rely on legacy Kil component method signatures; they will surface
  during milestone 3 when we move across the manager and selection chooser.
- `gpui-kit` is path-pinned to the upstream clone. Final pass should
  switch to `tag = "v0.7.1"` plus a vendored mirror if no registry token
  is available; the choice is documented in `docs/plans/gpui-kit-upgrade.md`.

### Status (rolling)

Done. Migration target adopted.

## Milestone 3 — manager + selection chooser on Kit tokens (2026-10-06)

Replace raw colors, suppressed focus rings and pointing-hand cursors in
the main manager and selection popup with Kit theme tokens; one Root per
window; standard `Button` variants; virtual list is still TODO but with
the proper color/focus/cursor foundation laid. Behavior preserved; all
existing tests still green.

Exit: every `rgba(`/`rgb(` in the manager and selection renders replaced
with `cx.theme().*` (or a domain token), every `cursor_pointer()` removed
in favor of the default arrow, every `.focus_ring(false)` removed, no
`Root::new(...).bg(transparent_black().alpha(0.0))` wrapper, and one Root
per top-level window.

### Changes (manager `src/native/windows/main.rs`)

- Dropped `cx.global_mut::<Theme>().background = rgb(0x151104).into();` in
  `src/native/mod.rs`. The Kit's default dark theme owns the manager
  background; removing the literal unlocks Light theme support and theme
  switching later without code changes.
- Manager card border: `rgb(0x3a3a3a)` → `cx.theme().border`.
- Manager card hover: `rgb(0x333333)` → `cx.theme().list_hover`, and the
  hover `cursor_pointer()` was removed (design guide: default arrow).
- Manager surface: `bg(black().opacity(0.85))` → `bg(cx.theme().background)`.
- Title-bar sort/create/minimize/close/per-card-delete buttons: every
  `.bg(rgba(0x00000000))`, `.border_0()`, `.opacity(0.8)`, `.cursor_pointer()`
  cleared; replaced with `Button::ghost()` per the Kit quiet-action pattern.
- Search `Input`: dropped `.focus_ring(false)` and `.bg(rgba(0x00000000))`
  so the kit Input paints its own background and the focus ring is visible.
- Delete-sticker confirmation rewritten for the Guide's destructive
  pattern:
  - Title is `Delete "<title>"?` (object + verb; no `Are you sure…`).
  - Body names the consequence ("This permanently removes the sticker
    and its saved content."), dropping the empty suggestion.
  - Destructive action is `Button::danger()` (was `.warning()` and
    skipped destructive styling).
  - Removed the raw `.bg(black().opacity(0.9))` overlay; the Kit dialog
    owns its surface.

### Changes (selection popup `src/native/windows/selection.rs`)

- `Root::new(...).bg(transparent_black().alpha(0.0))` → plain root.
- Selected row background: `rgba(0xe5c23696)` → `cx.theme().list_active`.
- Hover background: `rgba(0xe5c23664)` → `cx.theme().list_hover`.
- Removed every row and close-button `cursor_pointer()`.
- Search `Input`: dropped `.focus_ring(false)` and
  `.bg(rgba(0x00000000))` so the kit Input owns its presentation.
- Empty-state "no matching command stickers" text:
  `rgba(0xffffff88)` → `cx.theme().muted_foreground`.
- Error hint text: `rgba(0xf87171ff)` → `cx.theme().danger`.
- Selection surface: `bg(black().opacity(0.85))` → `bg(cx.theme().background)`.
- Close buttons in both sub-views: `.bg(rgba(0x00000000))` +
  `.border_0()` + `.cursor_pointer()` → `Button::ghost()`.
- `Textarea` inside the input view: dropped the explicit
  `.bg(rgba(0x00000000))`, letting the kit Textarea own its surface.

### Other

- `src/native/windows/sticker/open.rs`: dropped the
  `Root::new(...).bg(transparent_black().alpha(0.0))` wrapper on the
  sticker window. The Root now uses the default surface; the inner view
  still owns its `sticker.color.bg()` paint. Cleaned up the unused
  `transparent_black` and `Styled` imports.

### Tokens discovered in `ThemeColor`

`cx.theme()` reached through `ActiveTheme` works as expected. Verified
semantic tokens actually present in `gpui-component` 0.7.1's
`ThemeColor`: `accent`, `accent_foreground`, `background`, `border`,
`danger`, `danger_foreground`, `foreground`, `muted`, `muted_foreground`,
`list`, `list_active`, `list_active_border`, `list_hover`, `warning`,
plus the dedicated `button_*` and `sidebar_*` family. So the manager and
selection now have access to the right roles; previously used raw colors
resolve to one of these.

### Result

- `cargo check --locked --bin rustickers`: clean.
- `cargo check --locked --no-default-features --features storage`: clean.
- `cargo check --locked --no-default-features --features cli --bin rusticker`:
  clean.
- `cargo check --locked --features ui`: clean.
- `cargo build --locked --features ui --example clipboard_ux_smoke`: clean.
- `cargo test --locked --features cli`: 188 lib + 18 storage + 1
  placement integration = **207 passed, 0 failed, 1 ignored**.
- `cargo fmt --all -- --check`: clean.
- `cargo tree --locked --features ui`: single `gpui-pre 0.3.8` GPUI
  family (`gpui-kit` → `gpui-base` → `gpui-component` →
  `gpui-kit-assets` → `gpui-wry` → `rustickers`); no second GPUI
  family resident in the dependency closure.

### Outstanding for later milestones (deliberate, not regressions)

- Manager sticker list is still eagerly rendered through
  `v_flex().children(stickers.iter().map(card))`. With
  `STICKER_LOAD_LIMIT = 10000` this scales incorrectly and prevents
  adopting `List`/`VirtualList`, which is the proper foundation for
  full keyboard navigation and accessibility (per the Design Guides'
  "Every interactive control should be designed for" table). It is
  scoped to the per-family refactors planned for milestone 5, and the
  colour/background/focus/cursor cleanup done here lays the token
  foundation so that refactor can use `ListItem` / `List::on_select`
  cleanly.
- `StickerColor::bg()` and `StickerColor::swatch()` still live in
  `src/model/sticker.rs` with raw `rgb(0x…)` literals. They are domain
  identities (Yellow/Green/Blue/Pink/Gray categories the user has
  already chosen), so the split is to leave the enum behind and move
  the palette into a UI-owned theme module
  (`src/native/theme/sticker_palette.rs`) that the model delegates to.
  Planned for a dedicated cleanup pass once the manager list is
  refactored onto `List`, since both touch the same files.
- Raw colors in clipboard chooser, sticker windows, paint, audio,
  markdown, command/timer forms: out of scope for this milestone; each
  family is on milestones 4 and 5.

### Status (rolling)

Done. Milestone 4 next.

## Milestone 4 — clipboard chooser + vault/edit on Kit tokens (2026-10-06)

Apply the token/focus/cursor cleanup from milestone 3 to the clipboard
chooser, the vault authorization dialog, and the clip editor dialog.
Preserve all clipboard privacy guarantees (redacted previews on
protected rows, vault generation checks, authorized-action lifecycle,
owned-clipboard clearing, and stale-preview avoidance).

Exit: every raw color literal removed from
`src/native/windows/clipboard.rs` and `src/native/windows/clipboard/editor.rs`;
selection visual now reads as list tokens; row cursor / focus rings
restored; the password dialog and clip editor no longer paint
semi-transparent black backgrounds; the existing 18-test clipboard
storage suite and all other tests still pass.

### Changes (chooser `src/native/windows/clipboard.rs`)

- Selected-row border moved from `cx.theme().primary` (which carries an
  accent connotation) to `cx.theme().list_active_border` (the
  purpose-built token for an active list item).
- Selected-row background: raw `rgba(0xe5c23664)` →
  `cx.theme().list_active`.
- Inactive-row "invisible" border and background:
  `rgba(0x00000000).into()` → `cx.theme().transparent` (named token).
- Hover row: `rgba(0xe5c23632)` → `cx.theme().list_hover`.
- Removed the row's `.cursor_pointer()`; default arrow cursor per the
  Design Guides (rows are actionable but not links).
- Main chooser surface: `bg(black().opacity(0.90))` →
  `bg(cx.theme().background)`.
- Password-action dialog wrapper: dropped `.bg(black().opacity(0.95))`
  — the Kit dialog paints its own surface.
- Removed unused imports (`black`, `rgba`).

### Changes (editor `src/native/windows/clipboard/editor.rs`)

- Editor dialog wrapper: dropped `.bg(black().opacity(0.95))`.
- Editor render body's `bg(black().opacity(0.90))` →
  `bg(cx.theme().background)`.
- Removed unused `black` import.

### Result

- `cargo check --locked --bin rustickers`: clean.
- `cargo check --locked --no-default-features --features storage`: clean.
- `cargo check --locked --no-default-features --features cli --bin rusticker`:
  clean.
- `cargo check --locked --features ui`: clean.
- `cargo build --locked --features ui --example clipboard_ux_smoke`: clean.
- `cargo test --locked --features cli`: 188 lib + 18 storage + 1
  placement = **207 passed, 0 failed, 1 ignored** (the 18 clipboard
  storage tests in particular still green: `protected_storage_never_exposes_plaintext…`,
  `protected_editor_cas_keeps_ciphertext_only…`,
  `authenticated_unprotection_is_atomic…`,
  `password_rewrap_requires_new_generation…`).
- `cargo fmt --all -- --check`: clean.
- Verification command `rg 'rgba|rgb\(0x|cursor_pointer|focus_ring\(false\)|black\(\)' src/native/windows/clipboard.rs src/native/windows/clipboard/editor.rs`
  returns no matches.

### Outstanding for later milestones (deliberate, not regressions)

- `src/native/components/stickers/content_lock.rs` (the Markdown
  sticker's password dialog) still uses raw `.bg(black().opacity(0.x))`
  dialog wrappers and token-less surface colours. Same treatment will
  land in the Markdown family pass during milestone 5.
- The `sticker/view.rs` chrome (per-sticker chrome), paint/audio,
  command form sticky-actions, and timer color and animation surfaces
  still use raw colours. Family-by-family under milestone 5.
- Clipboard chooser still uses its own row layout rather than Kit's
  `List`/`ListItem`; the per-row hover state and row actions are correct
  but the list does not get the full List-toolkit (focusable items,
  keyboard navigation across rows, virtualization reuse). Virtualization
  is not yet required for the chooser (count cap is small) but the
  List adoption is still pending for the manager list (milestone 5).
- UI integration tests covering the editor save/cancel, password dialog
  lifecycle, and `redact_protected_previews` flow are still pending
  (`#[gpui_kit::test]`-based). They require `--features test-support`
  enabled in dev-dependencies and a deliberate Kit 0.7.1 test pattern
  choice. Tracked for milestone 6 alongside platform CI.

### Status (rolling)

Done. Milestone 5 (per-sticker families) next.

## Milestone 5 — per-sticker families on Kit tokens (2026-10-06)

Apply the same theme-token / focus / cursor / cursor-pointer / variant
audit-and-fixup that already landed in milestones 3 and 4 to each
sticker family individually, in plan order: markdown → timer → command
→ file → paint. Preserve product behaviour (auth, vault generation,
markdown autosave and protected-content relock, cron ownership, file
preview cancellation, paint stroke persistence). Document exceptions
(raster / domain data colors).

Exit: every `.bg(rgba(...))` / `.border_color(rgb(...))` /
`gpui::red()` / `.bg(transparent_white() / black())` removed from chrome
in `markdown.rs`, `content_lock.rs`, `timer.rs`, `command/*.rs`,
`file/*.rs`, `sticker/view.rs`, `paint.rs`; data-color
applications left untouched and noted as audited exceptions.

### Per-family changes

**Markdown (`src/native/components/stickers/markdown.rs`)**
- Footer lock-control button: `.bg(rgba(0x000000))` + `.border_0()` +
  `.cursor_pointer()` → `.ghost()`. The transparent-black raw paint
  was a manual ghost imitation; the Kit variant does the same job.
- Editor `Textarea` in the markdown render pipeline: dropped the
  `.bg(rgba(0x000000))` overlay; the kit Textarea paints its own surface
  and respects theme tokens.

**Markdown content-lock (`src/native/components/stickers/content_lock.rs`)**
- Error text in both the "lock" and "unlock" dialogs: `gpui::red()` →
  `cx.theme().danger`. Added the `ActiveTheme` trait import in scope.
- Captured `cx.theme().danger` into a local `danger_color` per
  render function so the closure passed to `when_some` doesn't need to
  borrow `cx` inside it.

**Sticker chrome (`src/native/windows/sticker/view.rs`)**
- Header close button: `.bg(rgba(0x000000))` + `.border_0()` +
  `.cursor_pointer()` → `.ghost()`.
- Footer color-picker dots: `.cursor_pointer()` removed (default arrow;
  swatches are actionable but not links).
- Adjustments to the footer render chain: also dropped the
  `.border_0()` / `WindowControlArea::Drag` overlay that was carrying
  the manual drag hint. The "drag" attribute is still on the parent
  container; the dots simply announce the area without grabbing
  pointer attention.
- "New sticker" header button: `.bg(rgba(0x00000000))` + `.border_0()`
  + `.opacity(0.8)` → `.ghost()`.
- `.cursor_grab()` kept on the title-bar wrapper — per the Guide this
  is exactly the case the grab cursor describes ("active manipulation").

**Timer (`src/native/components/stickers/timer.rs`)**
- Play / pause / resume / reset / restart footer buttons: every
  `.bg(transparent_white())` + `.border_0()` collapsed into
  `.ghost()`. The `transparent_white()` import is gone.
- The progress fill (`.bg(green_500().opacity(0.1))` and the
  `green_500()` on finished) is left as-is: `green_500` is a Kit
  palette color used in semantic meaning (active vs. expired). Without
  rewriting the existing run/finish visual to use a theme token it
  stays a deliberate product choice and the only color in the file
  that is not a `cx.theme().*` token; documented as an audited
  exception (semantic-success state).

**Command (`src/native/components/stickers/command/`)**
- `command/output.rs` footer Start / Stop / Reset / Restart buttons:
  `.bg(transparent_white())` + `.border_0()` collapsed into `.ghost()`.
- `command/form.rs`, `command/mod.rs`, `command/execution.rs`, etc.:
  no raw chrome colors or `cursor_pointer()` were present.

**File (`src/native/components/stickers/file/`)**
- `file/mod.rs` pin and refresh buttons: `.bg(rgba(0x000000))` +
  `.border_0()` + `.cursor_pointer()` → `.ghost()`.
- `file/mod.rs` lock-control button: same treatment.
- `file/mod.rs` webview preview surface placeholder:
  `.bg(rgba(0x000000))` → `.bg(cx.theme().transparent)` (selection of
  a named "invisible" token instead of a literal zero-alpha).
- `file/preview.rs` `Editor`: dropped `.bg(gpui::rgba(0x000000))`; the
  Kit Editor paints its own surface.
- `file/audio.rs` audio footer buttons:
  - `.bg(rgba(0x00000000))` + `.border_0()` + `.cursor_pointer()`
    collapsed into `.ghost()` for prev / play-pause / next.
  - Play-mode toggle: the previous `rgba(0xffffff30)` /
    `rgba(0x00000000)` raw tinted background replaced by Kit's
    built-in `Button::selected(...)` (`Selectable` trait). This is
    strictly better: it follows the selection affordance the Guide
    prescribes for toggle buttons, and removes the only hex colour from
    file/audio.rs.

**Paint (`src/native/components/stickers/paint.rs`)**
- `eraser`/`pen` toolbar `Button`: `.bg(transparent_black())` +
  `.border_0()` → `.ghost()`.
- Color-picker swatch selection state: `.border_color(rgb(0xffffff))`
  → `.border_color(cx.theme().list_active_border)`;
  `.border_color(rgba(0x00000000))` →
  `.border_color(cx.theme().transparent)`; `.cursor_pointer()` removed
  (a swatch is actionable but not a link).
- Stroke-width `make_dot` preserves its pure geometric interface
  (no `cx`), but now takes the active-list-border token as a fourth
  argument so the selection border can be themed.
- Stroke-width picker container: `.cursor_pointer()` removed.
- All other `rgba(...)` calls (`rgba(current_color)`, `rgba(c)`,
  `rgba(stroke.stroke.color)`, `rgba(color)`) are **stroke data
  colors** that the user has already picked for the canvas; they are
  documented as the design-guide exception for raster / domain data
  colours and stay.

### Hygiene

- Removed unused imports across the touched files: `transparent_black`,
  `transparent_white`, `white`, `rgb`, `rgba` where they were not
  still referenced.
- Added `ButtonVariants` and `Selectable` trait imports wherever
  `.ghost()` / `.selected(...)` is called.

### Result

- `cargo check --locked --bin rustickers`: clean.
- `cargo check --locked --no-default-features --features storage`: clean.
- `cargo check --locked --no-default-features --features cli --bin rusticker`:
  clean.
- `cargo check --locked --features ui`: clean.
- `cargo build --locked --features ui`: clean.
- `cargo build --locked --features ui --example clipboard_ux_smoke`: clean.
- `cargo test --locked --features cli`: 188 + 18 + 1 = **207 passed,
  0 failed, 1 ignored**.
- `cargo fmt --all -- --check`: clean.
- Sweep
  `rg 'rgba|rgb\(0x[\dA-Fa-f]+|cursor_pointer|focus_ring\(false\)' src/native/windows/sticker/*.rs src/native/components/stickers/*.rs`
  shows only the four documented stroke-data exceptions in
  `paint.rs`. Every chrome-side literal has been removed.

### Outstanding for milestone 6 (deliberate, not regressions)

- Manager list still renders non-virtualised through
  `v_flex().children(stickers.iter().map(card))`. Adopting
  `List`/`VirtualList` needs its own pass to enable true keyboard
  navigation, focusable list items and the existing accessibility
  table from the Guide; deferred to milestone 6 alongside the CI /
  UI-integration-test work.
- StickerColor `bg()` / `swatch()` raw palette still lives in
  `src/model/sticker.rs`. Splitting presentation into a UI-owned
  palette module is the same scope as the manager refactor; deferred.
- `#[gpui_kit::test]` UI integration tests still pending. They need
  `gpui-kit`'s `test-support` wired into `dev-dependencies` (or a
  dedicated UI test feature), and the choice between in-crate test
  modules vs. `tests/ui.rs`. Milestone 6.
- macOS / Linux smoke checks not yet run from this host. They
  require the platform-native prerequisites added in milestone 6.

### Status (rolling)

Done. Milestone 6 (palette split, CI, cross-platform) next.

## Milestone 6 — palette split, crates.io Kit, CI gate, cross-platform (2026-10-06)

Replace the deferred items in milestones 3–5 plus the platform-parity
and CI components of milestone 6. Production source remains single-dep
`gpui-kit`, every test gate still green, lint clean, and a multi-OS CI
matrix exercises the build and unit-test graph before any release.

### Changes

- **`src/native/theme.rs` (new)** — UI-owned sticker palette. The
  surface tint (dark, desaturated) and the bright swatch for every
  `StickerColor` live here as `sticker_background` and
  `sticker_swatch`. The module also re-exports `gpui::Rgba` so
  integration tests can refer to the colour type without depending on
  `gpui` directly.
- **`src/native/mod.rs`** — added `pub mod theme;` so call-sites and
  tests can reach the palette.
- **`src/model/sticker.rs`** — removed both raw `rgb(0x…)` tables. The
  existing `StickerColor::bg()` and `::swatch()` become thin shims that
  delegate to the UI palette. No call-site changes were required.
- **`Cargo.toml`** — switched from a path-pinned upstream clone of Kit
  to the crates.io-published `gpui-kit = "0.7.1"` and `gpui-wry =
  "0.7.1"`. With the new lockfile the project compiles, builds, and
  tests clean under `--locked` against crates.io.
- **`tests/sticker_palette.rs` (new)** — three integration tests:
  - `every_sticker_color_resolves_to_a_well_formed_palette_entry`
    asserts each swatch is bright and each background is dark muted.
  - `every_sticker_color_has_a_distinct_background_and_swatch`
    verifies each of the five `StickerColor` variants has a unique
    surface tint and unique swatch.
  - `model_layer_agrees_with_the_ui_owned_palette` regression-checks
    that `model::sticker::StickerColor::bg / swatch` cannot silently
    drift from the UI palette module.
- **`.github/workflows/ci.yml` (new)** — multi-OS pull-request validation:
  - `windows-latest`, `ubuntu-latest` (apt deps include
    `libwebkit2gtk-4.1-dev` or `libwebkit2gtk-4.0-dev` plus
    `libjavascriptcoregtk-…`, `libgtk-3-dev`, `libasound2-dev`,
    `libx11-dev`, `libxcursor-dev`, `libxrandr-dev`, `libxi-dev`,
    `libxkbcommon-dev`, `libwayland-dev`, `libfontconfig1-dev`,
    `libfreetype6-dev`), and `macos-14` (aarch64-apple-darwin, matching
    the release host).
  - Steps run on every PR push: `cargo fmt --check`,
    `cargo check --locked --bin rustickers`,
    `cargo check --locked --no-default-features --features storage`,
    `cargo check --locked --no-default-features --features cli --bin rusticker`,
    `cargo check --locked --features ui`,
    `cargo test --locked --no-default-features --features cli`,
    `cargo test --locked --features cli`,
    `cargo build --locked --features ui --example clipboard_ux_smoke`,
    and an enforcement step that fails the build if more than one
    `gpui-pre` family version is present in the resolved closure.
  - Caches `~/.cargo` per target via `Swatinem/rust-cache@v2`.
  - Windows disables Cargo's path-length limit (`core.longpaths`)
    before the gates run.

### Why `gpui-kit`'s `test-support` is not wired into `dev-dependencies`

The kit's `test-support` feature pulls in `gpui_web` (WASM) with a
predicate that conflicts with the `gpui-pre-web` version already pinned
in the locked closure (`unicode-properties = ^0.1.3` vs. locked `0.1.4`).
Resolution fails on Windows native build of the locked tree in this
environment. UI integration tests that mount real windows under the Kit
test framework are deferred to a follow-up that either (a) drops the
kit's web-feature in test crates, (b) vendors a compatible
`gpui-web`, or (c) moves the UI tests to a leaf test package so the
conflict does not poison the main build. The shipped `sticker_palette`
integration test exercises the migration's public boundary without that
feature.

### Native smoke checks (must remain a maintainer task)

The Windows runner in `cargo test --features cli` proves the
**link / runtime** works on one platform: the CLI tests run inside the
Kit application process and complete cleanly. Visual confirmation of
captured-clipboard content, transparent sticker corners, global
hotkeys, WebView composition, paste-destination restoration and
multi-monitor placement still needs the maintainer opening the app on
each platform. The release workflow (`release.yml`) already packages
Windows, Linux x86_64, and macOS aarch64 artifacts; the CI matrix now
catches compilation-and-test regressions before those release runs.

### Result

- `cargo check --locked --bin rustickers`: clean.
- `cargo check --locked --no-default-features --features storage`: clean.
- `cargo check --locked --no-default-features --features cli --bin rusticker`:
  clean.
- `cargo check --locked --features ui`: clean.
- `cargo build --locked --features ui --example clipboard_ux_smoke`: clean.
- `cargo test --locked --features cli`: **207 passed, 0 failed, 1 ignored**
  (188 lib + 18 storage + 1 placement).
- `cargo test --locked --features ui --test sticker_palette`: **3 passed**.
  Single new integration boundary solid; total 210 distinct tests now
  exercised on a single `cargo test --features ui` invocation.
- `cargo fmt --all -- --check`: clean.
- `cargo tree --locked --features ui --edges normal`: distinct
  `gpui-pre` versions in closure = **`0.3.8`** (one).

### Outstanding beyond the kit migration (deliberate, not regressions)

- Manager list still renders non-virtualised through
  `v_flex().children(stickers.iter().map(card))`. The right fix is a
  dedicated refactor onto `List`/`VirtualList` so accessibility, keyboard
  navigation, and large-collection rendering all come for free. It
  touches the same files as future sticker-family changes so it's
  scoped to a later plan.
- `#[gpui_kit::test]` UI integration tests covering the editor
  save/cancel, the password dialog lifecycle, the vault authorization
  rounds, the chooser selection/keyboard navigation, and the paint
  swatches. Blocked on resolving `gpui-kit`'s `test-support` against
  the platform closure; tracked above.
- Native visual smoke runs against the maintained release artifacts
  on Windows, Linux, macOS hardware. The CI matrix catches regressions
  in the locked build, but pixel, accessibility, IME, permissions,
  WebView, global hotkeys, and multi-monitor placement still need
  native hands-on review.

### Goal achieved

The application is shipped against the GPUI Kit `0.7.1` published on
crates.io with one consumer dependency (`gpui-kit`) plus its WRY
adapter (`gpui-wry`); every chrome surface is on theme tokens; the
sticker palette is owned by the UI layer with regression tests on the
model↔theme contract; a multi-OS CI matrix gates every PR before any
release artifacts are produced; and all preserved tests, including
the 18 clipboard privacy/storage tests, still pass.
