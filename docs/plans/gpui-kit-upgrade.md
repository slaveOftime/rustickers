# Whole-app GPUI Kit upgrade: investigation and plan

Status: proposed; application implementation has not started.

## Scope and invariants

Upgrade the GUI to the latest verified tagged GPUI Kit release and bring every application surface into conformance with its Coding and Design Guides. Preserve the existing product: a lightweight sticker manager, independent document/utility windows, scheduled command execution, file previews, paint, and a keyboard-first clipboard chooser. Do not turn it into a dashboard or introduce a dock merely because Kit provides one.

Non-negotiable regression boundaries:

- Existing SQLite databases, serialized sticker content, sticker IDs, CLI commands, and placement records remain compatible. No schema migration is required merely to update the UI toolkit.
- Clipboard collection remains paused by default. Protected content must not leak through previews, search, accessibility properties, logs, test snapshots, or stale asynchronous completions.
- Preserve clipboard destination restoration, copy-only versus paste behavior, vault generation checks, automatic relocking, and owned-clipboard clearing.
- Preserve monitor-aware placement, transparent sticker corners, resizing, pin/topmost behavior, global shortcuts, single-instance IPC, and platform permission handling.
- Preserve background schedules when their windows close, process termination, streamed results, and hidden-window error recovery.
- The CLI-only and storage-only builds must remain independent of UI dependencies.

## Installation and sources

Read `README.md`, `ARCHITECTURE.md`, the local GPUI Kit Coding Guides and component conventions/recipes, and the Design Guides in full.

Installed the requested skill with the non-interactive Pi target:

```sh
npx skills add https://github.com/longbridge/gpui-kit --skill gpui-kit-design-guides --agent pi --yes
```

Installation succeeded at `.pi/skills/gpui-kit-design-guides/`. Existing `.agents/`, `skills-lock.json`, and clipboard guide/plan files predated this work and must be preserved.

Primary upstream evidence:

- [Release v0.7.1](https://github.com/longbridge/gpui-kit/releases/tag/v0.7.1), commit `87d10ae5e1299d1be18670a37570c5402f952e34`.
- [Tagged workspace manifest](https://github.com/longbridge/gpui-kit/blob/v0.7.1/Cargo.toml): Kit family version 0.7.1; exact `gpui-pre-*` snapshot 0.3.8; platform enables `font-kit`, X11, Wayland, and runtime shaders.
- [Tagged Kit manifest](https://github.com/longbridge/gpui-kit/blob/v0.7.1/crates/kit/Cargo.toml): default component/assets features; opt-in test support.
- [Tagged Kit façade](https://github.com/longbridge/gpui-kit/blob/v0.7.1/crates/kit/src/lib.rs): GPUI root exports, `base`, `component`, `assets`, `platform`, Kit-aware `actions!`, `init`, and `open_window`.
- [Tagged WRY manifest](https://github.com/longbridge/gpui-kit/blob/v0.7.1/crates/webview/Cargo.toml): `gpui-wry` 0.7.1 uses the same GPUI workspace dependency and `lb-wry` 0.53.3.
- [Getting started](https://gpui-kit.com/docs/getting-started), [Coding Guides](https://gpui-kit.com/docs/coding-guides), [Design Guides](https://gpui-kit.com/docs/design-guides), [Testing](https://gpui-kit.com/docs/test). Checked documentation/source in the upstream clone when remote documentation requests failed.

Upstream was cloned to `C:/Users/binwenw/AppData/Local/Temp/rustickers-gpui-kit-upstream` for inspection. Observed main HEAD `c0bebdc8a8cf9a74f9f52feaca278a52a9cfcca2` still labels itself 0.7.1 but has substantial post-tag changes, including optional GPUI Fast. Do not mistake main documentation or its APIs for the tagged release. Verify any API used against the selected version.

Registry publication could not be independently confirmed: crates.io API returned HTTP 403, and the local offline registry has no `gpui-kit` entry. An initial `cargo info` inside this repository started fetching the existing Zed dependency; it was interrupted before baseline commands ran. This is not evidence of an application failure.

## Dependency decision

Recommended target: tagged 0.7.1, not floating main. Prefer exact registry requirements for `gpui-kit` and `gpui-wry` if registry resolution confirms both releases exist and align. Otherwise pin both to the verified release commit in `https://github.com/longbridge/gpui-kit`. Use one consistent source strategy for the Kit family; do not mix a Git WRY package with a registry façade without proving GPUI identity.

Replace direct `gpui`, `gpui_platform`, and `gpui-component` dependencies with optional `gpui-kit`, enabled only by `ui`. Application imports should use `gpui_kit`, `gpui_kit::component`, `gpui_kit::base`, and `gpui_kit::assets`. Retain `gpui-wry` as a separate optional adapter; Kit does not re-export it. Retain direct `lb-wry` only where the app genuinely constructs a native WRY builder, using the adapter-compatible version.

Use the default upstream engine. GPUI Fast is not in the tagged Kit manifest and adds a second migration axis. Do not add it in this upgrade.

Update `Cargo.lock` deliberately. Verify the dependency tree contains one active GPUI type family and no old Zed/longbridge Git dependencies. Inspect macro-generated crate paths, custom HTTP-client trait types, native raw handles, and macOS binding types during compilation; import replacement alone is not sufficient.

## Application audit

These are source-backed investigation findings, not a completed native visual review. The existing screenshot could not be visually evaluated in this environment.

| Area | Current evidence | Migration/design direction |
| --- | --- | --- |
| Bootstrap (`src/native/mod.rs`) | Separate platform setup and component initialization; forced dark mode and custom background | Kit application/init entry points; retain assets/HTTP integration; coherent system/light/dark theme policy rather than forced dark |
| Main manager (`src/native/windows/main.rs`) | Transparent custom chrome; hard-coded row styling; input focus ring disabled; eagerly rendered scrollable sticker rows; manually rendered dialog layer under a Root | Compact single-workspace manager: search and ordinary Add command, scannable sticker rows, explicit overflow/object commands; virtualize large collections; retain visible keyboard focus; one overlay owner |
| Shared sticker chrome (`src/native/windows/sticker/`) | Custom drag areas, pointer-hand commands, low-level platform placement and transparent Root | Keep independent sticker windows and native behavior; standard command buttons/menus, explicit accessible labels and focus; audit clipping and corner transparency |
| Selection popup (`src/native/windows/selection.rs`) | Custom rows, keyboard selection, suppressed input focus ring, literal selection/status colors | Focused utility chooser with search, visible selection, keyboard activation and cancellation; selection tracked by sticker identity; use standard list/menu behavior where it fits the selection payload contract |
| Clipboard (`src/native/windows/clipboard.rs`, `clipboard/editor.rs`) | Already uses `uniform_list`, retained input state, and extensive privacy controls; literal selected/hover fills and leading-edge selection border; manual dialog layer under Root | Preserve virtualization and security rather than rewrite the backend; themed row-surface selection, coherent command menu, visible capture/vault state; edit/protect/unlock decisions with focus restoration |
| Markdown (`components/stickers/markdown.rs`, `content_lock.rs`) | Retained editor, async saves/encryption, custom controls and transparent backgrounds | Preserve editor state through redraws and edit/preview changes; standard named commands; save failure recovery; protected content accessibility and relock tests |
| Timer (`components/stickers/timer.rs`) | Retained selects; fixed-pixel controls, unlabelled icon commands, direct green palette, repeated completion animation; timer loop requested from render | Labeled time controls, semantic progress/status and keyboard commands; move task lifecycle to construction/state transitions; bounded/cancellable loop and reduced-motion behavior |
| Command (`components/stickers/command/`) | Existing useful runner/schedule/execution boundaries; legacy `field`/`v_form`/Textarea; boolean controls use click toggles; cron subscription detached | Current documented Form/Field and input APIs; assign requested boolean values with change callbacks; retain subscriptions/tasks with explicit owners; clear configuration/run/error states and safe duplicate-submit handling |
| File/URL/audio (`components/stickers/file/`) | Preview entities, watcher tasks, custom mouse-oriented playback/edit controls; async preview/summaries | Preserve native previews and external-resource semantics; standard controls/menus; empty/missing/loading/error/read-only states; cancel or reject stale refreshes; keyboard playback and editor access |
| WebView (`components/webview.rs`) | Retained `Entity<gpui_wry::WebView>`; native WRY construction, transparent rendering, local protocol and external browser routing | Upgrade adapter alongside Kit; verify bounds, DPI, focus, hiding, destruction, local-file access and external navigation on real platforms |
| Paint (`components/stickers/paint.rs`) | Custom drawing elements and color data; mouse-oriented swatches/tools; literal UI selection boundary | Keep genuine drawing geometry and persisted color data; Kit tool controls, accessible swatch names, keyboard tool/undo paths, themed selection rather than literal white |
| Sticker palette (`src/model/sticker.rs`) | Domain color enum includes hard-coded GPUI presentation colors | Keep serialized color identities in model; resolve their presentation in a UI-owned theme/token layer, with light/dark contrast |
| Release workflow | Windows/Linux/macOS release builds; no separate PR UI validation workflow | Add feature/build/test gates and update native prerequisites from the selected Kit snapshot; validate actual platform behavior separately |

Raw drawing colors, raster dimensions, WebView geometry, platform insets, hit-test tolerances, and persisted physical window bounds can be legitimate exceptions. Document them. Ordinary UI spacing, typography, radius, selection and status colors are not exceptions.

## Phased implementation

### 0. Baseline and regression fixtures

- Record current feature checks/tests before dependency changes; preserve the initial working tree.
- Build representative fixtures using temporary storage: empty and large collections, long Unicode titles, every sticker type, missing files, failed commands, locked vault, stale protected edit, and multiple monitor placement records.
- Record native manual paths for hotkeys, title-bar drag/resize, pinning, WebView, and clipboard paste destination restoration.
- No redesign, schema changes, or unrelated dependency updates in this phase.

Exit: known baseline and fixtures; failures distinguished from tooling/environment limitations.

### 1. Dependency and façade migration

Files: `Cargo.toml`, `Cargo.lock`, all UI import sites (including feature-gated model/helpers and `examples/clipboard_ux_smoke.rs`).

- Confirm registry availability or use the verified release revision consistently.
- Replace dependency features/imports and Kit-aware macros; preserve CLI/storage feature isolation.
- Resolve actual GPUI/Kit API changes incrementally from compiler diagnostics and tagged source.
- Upgrade WRY adapter and check native/HTTP type compatibility before UI restructuring.

Exit: GUI and CLI compile; storage/CLI tests pass; dependency-tree audit passes. UI appearance remains intentionally close to baseline.

### 2. Shared window, theme, focus, and lifecycle foundation

Files: bootstrap, UI-owned theme/tokens, assets, shared chrome, all window openers.

- Initialize Kit once before entities/windows. Give each window exactly one Root. Prefer Kit `open_window`; retain explicit wrapping only where transparent-root configuration requires it.
- Remove duplicated application overlay-layer rendering. Verify dialogs, menus, notifications and sheets actually appear and dismiss through Root.
- Preserve native handles, bounds persistence, topmost behavior, drag-area occlusion and platform shortcuts. Avoid using the title as an implicit rename control.
- Centralize sticker palette roles and transparent-surface policy. Use semantic theme roles and rem/component sizes for ordinary UI. Preserve the product's color identities without giving arbitrary status meaning to them.
- Retain subscriptions and cancellable tasks in their owner. Distinguish app-lifetime supervisors from view-lifetime work; eliminate task creation from render. Audit save-on-close paths that currently schedule work before persistence finishes.

Exit: representative windows render in light/dark, overlays have one owner, keyboard focus survives open/dismiss, native smoke paths still work.

### 3. Main manager and selection chooser

- Primary tasks: find/create/open a sticker; select a compatible command for captured selection.
- Keep utility-window proportions rather than introduce unnecessary navigation. Define minimum/default/surplus size behavior.
- Virtualize large sticker collections with stable domain identity and zoom-aware measurement. Preserve selection when refresh/filter ordering changes where the selected object remains available.
- Provide ordinary visible Add/Open commands, object context menus and keyboard Actions. Secondary commands must not depend on hover.
- Restore search focus rings; distinguish selected, hovered, focused and disabled states. Align identity/status/action lanes and region-owned scrollbars.
- Make empty/loading/search-no-results/failure states actionable and precisely scoped.

Exit: UI integration tests activate real production handlers; long/large lists, keyboard selection and resizing pass.

### 4. Clipboard chooser and vault/edit flows

- Preserve the current virtualized architecture and all security-generation/CAS checks.
- Separate recent history, search results, pinned/protected state and capture status clearly; commands must describe the collection/object they affect.
- Replace literal row treatments and selection bar with semantic surface selection. Restore focus on topmost overlay cancellation.
- Standardize copy/paste/edit/protect/unprotect/delete menus and shortcut labels. Keep critical commands reachable without hover.
- Keep busy/validation/failure state near the decision; prevent repeated commits while preserving input. Use confirmations only for consequential irreversible operations.
- Ensure protected values never enter accessibility/test observation. Test lock or generation change while a dialog/task is pending.

Exit: existing privacy tests remain green; real UI interactions cover ordinary and protected paths with negative/stale cases.

### 5. Every sticker feature

Implement and verify one family at a time: Markdown/lock, timer, command, file/URL/audio/WebView, then paint.

For each family, state the task, owning entity/model, component composition and keyboard path before editing. Keep existing feature boundaries; do not introduce crates merely to rename modules.

- Markdown: input retention, edit/preview/save, password dialogs, relock and failure recovery.
- Timer: current input/select APIs, labeled accessible commands, task ownership, pause/resume/restart persistence, expiry and reduced motion.
- Command: Form/Field, controlled change callbacks, validation next to fields, streaming/run/stop/busy state, cron ownership, hidden-window recovery and background handoff.
- File/audio/WebView: watcher/preview cancellation, missing/offline/error views, accessible playback/source actions, local versus external navigation, native surface lifecycle.
- Paint: accessible tool palette and canvas instructions, keyboard commands and save feedback; keep actual stroke colors and geometry as data.

Exit per family: relevant existing tests plus production-view UI tests and native interaction checks pass; no feature is left on compatibility-only styling.

### 6. Whole-app verification, CI and documentation

- Add PR validation separate from release uploading. Verify Windows x86_64, Linux x86_64, macOS arm64 using their actual toolchains/native prerequisites; include platform-specific permissions and prerequisites in README.
- Run the Design Guides' full accessibility/design checklists on every surface, not just the manager.
- Update README/architecture only to reflect delivered behavior/dependencies. Replace screenshots only after native review.

Exit: feature matrix and regression matrix complete, with any unavailable native verification stated explicitly.

## Verification matrix

Automated gates (use `--locked`; offline only when the selected dependencies are cached):

```sh
cargo fmt --all -- --check
cargo check --locked --bin rustickers
cargo check --locked --no-default-features
cargo check --locked --no-default-features --features storage
cargo check --locked --no-default-features --features cli --bin rusticker
cargo test --locked --no-default-features --features cli
cargo test --locked --features cli
cargo check --locked --examples
cargo tree --locked -d
```

Add Kit `test-support` from exactly the normal dependency's source/version. Verify that a development dependency does not inadvertently defeat CLI-only feature isolation; if necessary expose test support through a dedicated UI test feature rather than globally activating an optional GUI dependency. Write `#[gpui_kit::test]` with explicit type imports, avoiding a glob that shadows ordinary Rust `#[test]`.

UI integration tests must mount production views under Root, dispatch actual click/key/input events, and assert visible state, focus/layout and application outcomes. Cover retained text after unrelated redraw, invalid submission, disabled/busy rejection, overlay Escape/focus return, stable selection, virtual scrolling, task cancellation and stale completion. Use injected storage/platform seams where needed, not duplicated fixture-only UI.

Native verification remains separate: headless tests do not establish pixels, OS clipboard injection, system IME, accessibility quality, permission prompts, WebView composition, global hotkeys or multi-monitor placement. Review light/dark, larger base fonts, minimum/default/wide windows, display scaling, long/CJK content, reduced motion and failure states on each supported OS. Measure intended alignment and repeated gaps from resolved bounds instead of approving a casual screenshot.

## Baseline results

Before migration:

- `cargo check --locked --offline --bin rustickers`: passed.
- `cargo test --locked --offline --no-default-features --features cli`: passed (97 unit tests, 18 clipboard storage tests, 1 placement integration test; 1 interactive terminal test ignored).
- `cargo fmt --all -- --check`: passed.
- `cargo test --locked --offline --features cli`: blocked during build because Cargo could not remove `target/debug/rustickers.exe` (`Access is denied`, Windows error 5). A running application or another file lock may be responsible; this is not a test assertion failure. Do not terminate the user's app automatically. Close/release the executable or use an isolated target directory before re-running the UI-enabled baseline.

## Approval boundary

This document completes the investigate-and-plan stage. No application source, manifest or lockfile migration has been made. Recommended next step: approve the tagged-release target and phased scope, then implement phase 1 before any visual redesign. Re-check the latest release at implementation time; if a newer release appears, update this evidence and compatibility gate rather than silently changing the target.
