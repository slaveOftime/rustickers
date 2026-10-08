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
| `src/native/windows/text/picker.rs` | TXTs list, filters, hover actions, and focus dismissal |
| `src/native/windows/text/{password,title}.rs` | Shared authorization/settings and title dialogs |
| `src/native/windows/text/platform.rs` | Native picker raising; separate from focus and domain actions |
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
- Drag regions swallow pointer events before ordinary controls. Add `.occlude()` to Buttons and control containers in **both headers and footers**. Input does not expose that method: wrap it in `div().occlude().child(Input::new(...))`. Parent chrome occlusion alone is not enough.

For popup UX, follow `selection.rs`: search first, content dominant, lightweight hints, and a short keyboard path. TXTs uses explicitly requested trailing hover actions, with equivalent keyboard commands; Copy/Paste are keyboard-only.

## State, events, and async work

Retain input entities, subscriptions, focus handles, and scroll handles in their owner. Rendering only reads state and builds elements: no I/O, mutation, task spawning, or notification loops.

Start work from an event or lifecycle method. Run blocking I/O/crypto off the GUI thread, update through weak entity handles, and tolerate a closed window. Polling loops must stop when the owner disappears; revision-tagged loops must stop when superseded. Reject stale results by domain identity/revision.

Use one owner method for each logical command across buttons, menus, and keyboard paths. Respect modal/menu focus; Escape dismisses the topmost surface and returns focus. Guard duplicate submissions, show recoverable errors, and never silently discard dirty encrypted drafts.

## TXTs contracts to preserve

- All / Text / Clipboard filters use the shared `TextRepository` projection. Rows show at most three lines; locked previews never contain plaintext.
- Save/Lock promotes captures into **closed** stickers; Edit opens the normal editor.
- Lock requires a public title; Rename uses the same required-title dialog. Locked stickers show a centered public-title field above Unlock; Enter commits its title. Never expose secret content as a locked preview.
- Unlock authorizes viewing, not editing: show Markdown preview, clear the edit snapshot, and leave dirty state false. Enter editing only through an explicit edit command; create its snapshot at that transition.
- Copy/paste does not open stickers. Without an external destination, Enter copies; otherwise it pastes. F2/double-click edits; Ctrl/Cmd+F2 renames. Hover actions must remain reachable through these equivalent keyboard commands.
- Close TXTs on window deactivation, even while busy. An authorized paste must be app-owned, not window-owned: closing the picker must neither cancel delivery nor resurrect the window after failure.
- Saved-item deletion requires confirmation; capture deletion is immediate.
- The manager, TXTs, and encrypted editor share one master-password system. No vault or cached global unlock state.
- Keep clipboard suppression, destination verification, and ownership-safe expiry in OS adapters. Do not claim unsupported platform parity.

## Native checks

Sticker windows use transparent backgrounds for corners. On Windows, startup sets `GPUI_DISABLE_DIRECT_COMPOSITION=true` unless already configured, so embedded WebViews paint correctly. Keep WebView construction fallible and expose errors rather than blank panels.

Verify resizing, DPI, keyboard navigation, focus restoration, long text, empty/error states, and light/dark themes in a real window. Compilation and repository tests do not prove rendered UI quality.

## Fast iteration and regression checks

1. Inspect the current working diff before editing; preserve user tweaks. Trace the real owner method from pointer, keyboard, and menu entry points.
2. For a dead click, check drag hit testing/occlusion, disabled/busy guards, modal guards, then callback/async completion. A working keyboard path does not prove a working button.
3. For clipped tail rows, bound the **viewport**, not the content: fixed header/footer use `flex_shrink_0()`, list body uses `flex_1().min_h_0().overflow_hidden()`, and `uniform_list` fills that body. Give every virtual row the same height; release long text's width with `min_w_0()`. Check the first and last item with wheel and keyboard navigation.
4. Multiline previews must preserve line breaks and Unicode in display data, then use `line_clamp(3)` in the view. A single-line sanitized preview cannot become multiline through styling. Budget row height for three text lines, metadata, actions, padding, and rem zoom.
5. Hotkey presentation needs both native z-order and activation. `WindowKind::Floating` alone does not guarantee topmost ordering; raise after native creation through `text/platform.rs`. Test from another app, including one with topmost windows, not only from the manager.
6. Use an isolated temporary database/window to exercise actual pointer paths. Assert the outcome (dialog displayed, content saved, encryption removed, window closed), not merely event delivery. Capture preview/edit/locked states, hover actions, and scroll tails in one bounded pass; fix in a batch, then confirm once.

Keep evidence precise: repository tests prove storage contracts; native-event probes prove their specific event paths; screenshots prove only what was inspected. A synthetic Windows `WM_ACTIVATE` check is not proof of normal foreground switching. If the host blocks foreground transfer or images cannot be inspected, report that limitation. Remove temporary probes, debug instrumentation, and stale documentation references before committing.

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
