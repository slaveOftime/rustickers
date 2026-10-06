# Temporary implementation plan: reliability and frontend improvements

Status: not started. Planning only; implementation requires a separate go-ahead.

This is a working checklist, not permanent project documentation. Keep durable
rules in `FRONTEND.md`, then delete this file when the work is complete.

## Goals and scope

Improve reliability first, then manager performance and keyboard ergonomics.
Work in independently validated phases; do not combine unrelated rewrites.

Preserve:
- CLI-only and storage-only builds, clipboard collection paused by default.
- Protected-content redaction, authorization and generation checks, atomic
  vault rewrites, and owned-clipboard clearing.
- Native placement/DPI, transparent corners, global hotkeys, single-instance
  IPC, WebView lifetime, and paste-destination restoration.

Read the local GPUI Kit Coding and Design Guides before changing behavior or
presentation. Verify every framework signature against the installed 0.7.1
source; documentation examples are not proof that an API exists.

Out of scope: dependency upgrades, generic event buses/listener wrappers,
blanket SQLite transaction changes, IPC protocol redesign, tray/coaching UI,
and a full light-theme rollout.

## Phase 0 — baseline and documentation accuracy

- [ ] Record working-tree state and run the validation commands below.
- [ ] Correct `FRONTEND.md` using current code and installed Kit source:
  task ownership versus window-context access, explicit markdown saving,
  existing panic hook, component imports, bootstrap and compilable examples.
- [ ] Remove unsupported implementation recipes and speculative performance
  claims rather than treating them as project conventions.
- [ ] Keep upstream skill snapshots unchanged; project rules belong in
  `FRONTEND.md`, not edits to external reference material.

Exit: handbook claims are source-backed; existing gates retain their baseline.

## Phase 1 — timer task ownership

Files: `src/native/components/stickers/timer.rs` and focused tests.

Verified issues:
- Detached tick loop ignores failed weak-entity updates, so it can continue
  waking after the entity disappears.
- Start/resume/restart reset a boolean guard without cancelling a prior loop.
- Rendering initiates ticking instead of describing state.

Implementation:
- [ ] Retain the ticking `Task` in the timer entity. Verify drop cancellation
  semantics in the installed GPUI implementation before relying on them.
- [ ] Centralize tick-task start/replace/stop in named lifecycle methods.
- [ ] Start ticking for restored running timers during initialization and for
  newly running timers during state transitions, not from `render`.
- [ ] Cancel the old task before replacement; exit on failed entity updates.
- [ ] Review beep-task ownership too: repeated completion/restart must not
  leave competing beep loops.
- [ ] Preserve elapsed-time calculation, persistence cadence and completion
  semantics; cancellation is not a change to timer storage behavior.

Tests: restored running timer, start/pause/resume/reset/restart, rapid repeated
transitions, exactly one active loop, entity release, and completion/beep reset.
Use deterministic time/executor controls where available; do not use sleep-based
flaky assertions. If GPUI test support cannot resolve, report the limitation and
cover pure transitions plus a manual lifecycle smoke test separately.

Exit: no tick startup from render, no competing tick tasks, teardown stops work.

## Phase 2 — manager request and command correctness

File: `src/native/windows/main.rs` and focused tests.

- [ ] Replace the loading-only request guard with a latest-request policy.
  Choose either revision-tagged concurrent results or one active request with
  a queued latest request; document the choice.
- [ ] Include query and sort order in request identity. Older responses must
  not overwrite newer intent, including failure/loading state.
- [ ] Preserve usable previous rows while refreshing.
- [ ] Route card opening through one owner method used by mouse, keyboard and
  menus. Surface failures; avoid leaving an optimistically open state on error.
- [ ] Notify after successful deletion and repair selection coherently.
- [ ] Ensure deletion during an in-flight load cannot resurrect a stale row
  in the displayed list.

Tests: out-of-order responses, sort/search changes while loading, latest request
failure, delete during refresh, open failure, and successful mutation notification.

Exit: displayed rows match the latest completed valid request; failures are visible.

## Phase 3 — manager virtualization and keyboard interaction

Files: `src/native/windows/main.rs`; extract a manager-local module only if it
makes state ownership or tests clearer.

- [ ] Inspect installed `List`/virtual-list interfaces and existing chooser
  usage before selecting a component.
- [ ] Account for wrapped titles and variable card heights. Use a variable-height
  implementation, or explicitly design uniform rows before using `uniform_list`.
- [ ] Separate domain rows, filter/sort intent, selected sticker ID and viewport.
- [ ] Render only visible rows with stable domain-derived element IDs.
- [ ] Add an explicit focus owner and contextual Actions: arrows, Home/End,
  page navigation where supported, Enter to open, and Delete through confirmation.
- [ ] Preserve selection by ID across sort/refresh; define fallback when the
  selected row disappears. Scroll keyboard selection into view.
- [ ] Share command handlers between pointer activation, keyboard and menus.
  Preserve existing double-click behavior. Add only commands the manager can
  actually perform; do not invent an F2 editing workflow.
- [ ] Keep focus, hover and selection visibly distinct; check focus-ring clipping.
- [ ] Assess the separate 10,000-row query ceiling. Either implement pagination
  with tests as a separate change or make the limit visible; virtualization alone
  must not be presented as removing it.

Validation: realistic fixtures at 100/1,000/10,000 rows, render-count or timing
measurements before/after, long titles, resizing/rem changes, sort/refresh/delete,
keyboard-only workflow, and native scrolling smoke. Report measurements rather
than claiming a fixed visible-row count or speedup.

Exit: rendering scales with the viewport; keyboard activation and selection remain
correct when offscreen rows are not rendered.

## Phase 4 — recoverable WebView construction

Files: `src/native/components/webview.rs`, `stickers/file/preview.rs`, and callers.

- [ ] Replace `build_as_child(window).unwrap()` with a fallible constructor.
- [ ] Propagate errors through `FilePreview::new` and the existing file error UI.
- [ ] Verify failure propagation through GPUI entity construction rather than
  inventing a fallible `cx.new` overload.
- [ ] Preserve focus-parent behavior, transparency, reload and resource lifetime.
- [ ] Do not indiscriminately replace invariant HTTP response-builder unwraps;
  distinguish external failures from fixed-value construction invariants.

Tests: construction failure propagation and readable recovery state. Native smoke:
local document, URL, video/PDF where supported, reload, close and reopen.

Exit: unavailable WebView backend does not terminate the app.

## Phase 5 — clipboard read efficiency and refresh coordination

Files: `src/native/clipboard/watcher.rs`, `src/native/windows/clipboard.rs`,
`src/clipboard/storage.rs`, and bootstrap wiring in `src/native/mod.rs`.

Verified cost: watcher reads full history every 250 ms to decide whether any
protected history exists; chooser reloads snapshots/settings every second.

- [ ] Introduce a narrow storage read for capture eligibility using settings
  and an existence query rather than loading content-bearing snapshots.
- [ ] Preserve the fail-closed pause for protected history, including CLI edits
  and cross-process writes. On read failure, collection must remain paused.
- [ ] Keep authoritative storage-side capture checks; a lightweight read is
  not authorization and must not introduce a check/write race.
- [ ] Replace unused payload notifications with content-free change signals
  if notifications are wired to the UI. Do not retain plaintext in a UI queue.
- [ ] Coalesce refresh requests and retain query revisions/stale-result rejection.
- [ ] Use local change signals where practical, plus bounded reconciliation for
  writes from other processes. Local watcher events alone are insufficient.
- [ ] Notify only when capture status or displayed state actually changes.

Tests: paused defaults, protected-history pause, settings/read failures, concurrent
protection changes, external CLI changes, coalesced refresh, teardown, and all
existing clipboard-storage regressions.

Exit: fewer content-bearing reads at idle without reducing privacy guarantees.

## Phase 6 — targeted ergonomic safeguards

- [ ] Design confirmation for irreversible removal of protected or deliberately
  retained clipboard items; avoid blanket modals for ordinary disposable history.
- [ ] Name scope/consequence without exposing protected content. Ensure keyboard
  and pointer removal follow the same policy and revalidate target identity.
- [ ] Add concise platform-correct shortcut discoverability, preferably through
  shared menus/help rather than an overloaded footer.
- [ ] Expose active sort meaning and loading/error state using existing Kit
  patterns. Do not add shadows or skeletons without a real task benefit.
- [ ] Keep palette additions tied to an actual theme requirement. Do not create
  `CardSurface` or other wrappers solely to bundle a few existing lookups.

Exit: keyboard-only workflow remains complete; errors and destructive outcomes
are understandable; no new privacy disclosures or unnecessary confirmation ritual.

## Validation and delivery

For each phase: focused tests first where feasible, then formatting, relevant
feature checks and regression suites. Run full gates at integration points:

```bash
cargo fmt --all -- --check
cargo check --locked --bin rustickers
cargo check --locked --no-default-features --features storage
cargo check --locked --no-default-features --features cli --bin rusticker
cargo check --locked --features ui
cargo clippy --locked --features ui
cargo clippy --locked --no-default-features --features cli
cargo test --locked --no-default-features --features cli
cargo test --locked --features cli
cargo build --locked --features ui --example clipboard_ux_smoke
```

- [ ] Confirm one `gpui-pre` version in the UI dependency graph; use the existing
  CI check or parsed Cargo metadata, not a grep expecting lockfile syntax in tree output.
- [ ] Record existing warnings separately from introduced warnings; fix the known
  match-like-matches lint when touching its file.
- [ ] Record automated versus manual evidence separately. Passing compilation
  does not prove focus, layout, native composition or platform parity.
- [ ] Validate relevant Windows/Linux/macOS behavior through CI and native smoke;
  report platforms not exercised rather than declaring parity from one host.
- [ ] Make cohesive commits after validation when requested. Update `FRONTEND.md`
  with verified durable changes, then remove this temporary plan.

Stop if resolving UI test support requires a dependency upgrade or privacy behavior
cannot be tested safely. Report the issue and choose a separately reviewed path.
