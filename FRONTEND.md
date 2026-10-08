# Frontend guide

Project-specific rules for the GPUI desktop layer. Start with [README.md](README.md) for usage and [ARCHITECTURE.md](ARCHITECTURE.md) for ownership/storage.

## Read before changing UI

The local framework guides are authoritative:

- [Design guides](.agents/skills/gpui-kit-design-guides/references/design-guides.md): layout, hierarchy, controls, copy, and interaction states.
- [Coding guides](.agents/skills/gpui-kit/references/coding-guides.md): entities, contexts, focus, async work, and tests.

Use the current crate source for exact APIs. Do not invent GPUI methods from web-framework patterns.

## Where to work

| File / folder | Owns |
| --- | --- |
| `src/native/mod.rs` | Bootstrap, theme, services, clipboard watcher |
| `src/native/windows/main.rs` | Sticker manager and event refresh |
| `src/native/windows/sticker/` | Window lifecycle, chrome, placement, close/save |
| `src/native/windows/selection.rs` | Compact search-first selection popup |
| `src/native/windows/clipboard.rs` | TXTs list, filters, selection, commands |
| `src/native/windows/text_{password,title}.rs` | Shared authorization/settings and title dialogs |
| `src/native/components/stickers/` | Type-specific rendering and editor behavior |
| `src/native/components/webview.rs` | Embedded browser lifecycle |
| `src/native/theme.rs` | Sticker background/swatch palette |

## Composition and styling

Use GPUI Kit's components through the project's `gpui` / `crate::gpui_component` aliases. Initialize components once and wrap each window in `Root`; Root owns dialog and popup layers.

- Read semantic colors from `cx.theme()`; use rem-based spacing and component sizes.
- Keep app chrome neutral. `StickerColor` identity lives in the model; its palette lives in `native/theme.rs`. Paint stroke colors are content, not chrome.
- Use Buttons for commands, primary for the default commit, ghost for quiet toolbar actions, and danger for irreversible confirmation.
- Use persistent selected states, stable domain-derived element IDs, and a visible keyboard focus ring.
- Let the primary list/editor fill the available space. Flexible children need `min_w_0()` / `min_h_0()`; each scroll region has one owner.
- Virtualize large lists with `uniform_list` and consistent row heights. Preserve selection by ID, not position.
- Drag-area inputs/buttons must `.occlude()` so window dragging does not swallow clicks.

For popup UX, follow `selection.rs`: search first, content dominant, lightweight hints, and a short keyboard path. Secondary commands belong in standard menus, not crowded button strips.

## State, events, and async work

Retain input entities, subscriptions, focus handles, and scroll handles in their owner. Rendering only reads state and builds elements: no I/O, mutation, task spawning, or notification loops.

Start work from an event or lifecycle method. Run blocking I/O/crypto off the GUI thread, update through weak entity handles, and tolerate a closed window. Polling loops must stop when the owner disappears; revision-tagged loops must stop when superseded. Reject stale results by domain identity/revision.

Use one owner method for each logical command across buttons, menus, and keyboard paths. Respect modal/menu focus; Escape dismisses the topmost surface and returns focus. Guard duplicate submissions, show recoverable errors, and never silently discard dirty encrypted drafts.

## TXTs contracts to preserve

- All / Saved / Clipboard filters use the shared `TextRepository` projection.
- Save/Lock promotes captures into **closed** stickers; Edit opens the normal editor.
- Lock requires a public title; Rename uses the same required-title dialog. Never expose secret content as a locked preview.
- Copy/paste does not open stickers. Without an external destination, Enter copies; otherwise it pastes. F2/double-click edits.
- Saved-item deletion requires confirmation; capture deletion is immediate.
- The manager, TXTs, and encrypted editor share one master-password system. No vault or cached global unlock state.
- Keep clipboard suppression, destination verification, and ownership-safe expiry in OS adapters. Do not claim unsupported platform parity.

## Native checks

Sticker windows use transparent backgrounds for corners. On Windows, startup sets `GPUI_DISABLE_DIRECT_COMPOSITION=true` unless already configured, so embedded WebViews paint correctly. Keep WebView construction fallible and expose errors rather than blank panels.

Verify resizing, DPI, keyboard navigation, focus restoration, long text, empty/error states, and light/dark themes in a real window. Compilation and repository tests do not prove rendered UI quality.

## Verification

```bash
cargo fmt --all -- --check
cargo check --locked --features ui
cargo check --locked --no-default-features --features storage
cargo check --locked --no-default-features --features cli --bin rst
cargo clippy --locked --features ui
cargo test --locked --no-default-features --features cli
cargo test --locked --features cli
```

Keep one compatible `gpui-pre` family in the dependency tree. Focused tests cover text repository/protection/titles/CLI, clipboard import, palette, placement, and scheduling. Use temporary databases.

GPUI Kit 0.7.1 has documented Windows headless/test-support limitations; do not label compile checks as UI integration coverage. For optional OS-level captures, see [the visual-check recipe](.agents/skills/gpui-visual-check/SKILL.md) and `scripts/screenshot.ps1`. Use an isolated test setup, not the user's live data.
