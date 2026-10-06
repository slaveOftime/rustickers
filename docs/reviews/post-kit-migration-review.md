# Post-migration code review — Rustickers after GPUI Kit 0.7.1

Read alongside `docs/plans/gpui-kit-upgrade.md` (audit), `docs/plans/gpui-kit-upgrade-goal.md`
(milestone-by-milestone log) and `docs/plans/clipboard-design-guides.md` for the relevant
ergonomic rules. Recommendations are **prioritised**, framed against the four lenses the
request asked for: efficiency, speed, reliability, and ergonomics.

---

## 0. Build toolchain — current state

- `cargo fmt --all -- --check` — **clean.**
- `cargo check --locked --features ui` — **clean, no warnings.**
- `cargo clippy --locked --features ui` — **one** warning (`match → matches!`
  macro at `src/native/components/stickers/file/mod.rs:588`).
- `cargo clippy --locked --no-default-features --features cli` — **clean.**
- `cargo test --locked --features cli` — **210 passed, 0 failed, 1 ignored** (the
  1 ignored is the interactive-terminal smoke; documented).

Only **one** clippy lint is offending. The codebase is otherwise warning-clean.

---

## 1. Performance / efficiency (largest gap)

The migration cleaned up tokens and overlays but did **not** tackle rendering cost. Two
specific paths produce visible lag on realistic workloads:

### 1.1 Manager sticker list (highest impact)

**Where:** `src/native/windows/main.rs` — `sticker_card`, render chain in `MainWindow::render`.

**Problem:**
- `STICKER_LOAD_LIMIT = 10000`, eagerly rendered through
  `v_flex().overflow_y_scrollbar().children(self.stickers.iter().map(...))`.
- Every frame rebuilds **10 000** `div`s, each with a closure-captured title, the
  result badge, the spinner, the loop glyph, the open-delete icon, and the
  click-listener.
- `cx.listener(move |this, ...| ...)` is generated **per row, per render** — every
  card carries a fresh `Rc` closure with its own captured title.

Per Design Guides: "Virtualize large collections without changing keyboard semantics."
The clipboard chooser follows this with `uniform_list`; the manager does not.

**Recommendation (do this next, before considering any further work):**

1. Switch the manager body to Kit's `List` (or `uniform_list` as the chooser does)
   so only the visible window of rows is rendered each frame.
2. Replace the per-card listener closure with `cx.listener(...)` captured
   *once* at construction, plus `click_count >= 2` dispatching to the
   `id` of the clicked row (the row passes its id through a typed
   `ElementId`).
3. Same change unblocks real keyboard activation (`List` exposes key
   navigation; the current implementation only listens to `MouseUpEvent`
   with `click_count >= 2`).

**Impact:** turns a 10 k-element render into a sub-100 element render; eliminates
the per-row closure allocation; makes the manager passable on slow machines.

### 1.2 Wait-for-input polling in clipboard chooser

**Where:** `src/native/windows/clipboard.rs` lines 408–420 (1-second timer that calls
`refresh` even when nothing changed).

The chooser polls each second. On idle windows this is fine; on busy windows the
1-Hz loop still walks the row list. The watcher (`watcher::WatcherHandle`) only
pushes on real change; `clipboard_ux_smoke.rs` polls for visual confirmation but
the production window could replace polling with an event-channel subscription on
the watcher. This is a small win but eliminates redundant SQL queries when no new
captures arrive.

### 1.3 String allocation hot paths

`rg '\.clone|format!|to_string|to_owned' src/native/ | wc -l` → **453** sites. Most
are harmless (`String` round-tripping) but a few are blocking events:

- `src/native/components/stickers/command/runner.rs` and `command/execution.rs`
  serialise every runner event into `serde_json::to_string` for each subscribed
  consumer; per-tick object churn. If TC bandwidth on a 100 k-IOPS stream matters,
  pass a typed enum over an mpsc instead.
- `src/native/components/stickers/markdown.rs` autosave serialises the full
  document per save; with 50 KB notes this is the right approach, but the
  per-keystroke `InputEvent::Change` callback should debounce and not block
  on saving.

### 1.4 Per-render listener clones

A pattern observed in every sticker: `cx.listener(move |this, ...| ...)` is
created **inside render** for each row, taking owned captures of `id` / `title` /
`window_handle`. The closures are short-lived — GPUI deduplicates by `ElementId` —
but the allocation cost is real. After virtualisation lands (1.1) this falls out
naturally because the closured `move` only happens for currently-visible rows.

---

## 2. Reliability / safety nets

### 2.1 Tasks spawned from `render()` / per-render guard

**Where:**
- `src/native/components/stickers/timer.rs:567` — `spawn_for_timer(cx)` is
  called from inside `Render::render`.
- `src/native/components/stickers/markdown.rs:631` and surrounding — the
  markdown editor's preview re-render task is started from `Render`.

The Kit Coding Guide and `gpui::*`'s contract say:
> `cx.spawn` from render is allowed only with a guard (idempotency / "have I
> already started this loop?"). Tasks must have lifetimes owned by an
> Entity, not the render call.

`Timer` does have a guard (`self.timer_loop_running`), which is good. The pattern
is correct **but** the loop has no cancellation token — when the window is closed
the task keeps running until the next break. With `TimerLoopRunning` set false at
the end, the next loop iteration will exit naturally — but the timer can keep a
sticker observed for an extra wake after window close. **Recommended:** switch
to `cx.spawn_in(window, async move ...)` and bind the loop to the window's
lifetime (`cx.on_window_close(...)` is a Kit hook).

The markdowns autosave loop is a stronger candidate for the same refactor — it
spawns `cx.spawn(async |...| loop { ... })` from inside a subscription handler
today; should ensure cancellation on view drop.

### 2.2 Reading the missing-window case in `selection.rs`

**Where:** `src/native/windows/selection.rs:286` — `clear_popup_handle()` calls
`window.remove_window()` after the Entity has been dropped. Closed-over captures
of `weak_entity` are guarded against upgrade failure; the rest of the lifecycle
is sound. The one subtle risk: `let popup = SELECTION_POPUP.read().ok().and_then(|popup| popup.take())`. If the
read-lock is held during a poisoned-panic shutdown, the write-lock elsewhere can
deadlock. Lock-free `AtomicCell<Option<AnyWindowHandle>>` would remove the
contention; trivial follow-up.

### 2.3 `panic!`/`unwrap` audit (34 sites, 0 in src/storage)

Most are in `src/clipboard/vault_controller.rs` (test code), `src/utils/logging.rs`
(startup-stage fail-fast with `.expect("…")` — correct usage), and one in `clipboard/argv.rs`
(test code). The runtime path is clean:

- The CLI's vault controller fails closed on startup if the on-disk envelope can't be
  opened (correct).
- `clipboard/watcher.rs` uses `?` and converts to `tracing::warn!` (correct).
- `wry::WebViewBuilder` constructor in `native/components/webview.rs` uses `.unwrap()` on
  `build_as_child(window)`; if the WRY backend fails to construct, the app
  crashes. Switch to `?`-based error propagation already started in
  `clipboard_ux_smoke.rs` (`Result`-flavored), or surface the error inline as a
  fallback preview.

### 2.4 IPC reconnect / single-instance leftovers

`ipc::SingleInstance::acquire` distinguishes live / corpse sockets via a probe
connect. **Recommended improvements:**

1. Heartbeat — when the primary instance is up, it should ack the probe so a
   fast-connecting probe doesn't race a corpse. The current implementation uses
   `connect_and_signal(name)`, which sends a single event; a 1-byte ACK suffices.
2. Socket cleanup at process exit — on `ctrl-c` or panic, the listening socket
   should be removed by name. The current code relies on OS cleanup, which works
   on Linux, fails on macOS with stale socket files.
3. Drop the "permission denied" branch's silent fallback: today the body hits
   `tracing::warn!` and continues with a non-listening `SingleInstance`. Make the
   decision explicit.

---

## 3. Reliability — concurrency

### 3.1 SQLite contention under heavy concurrent load

**Where:** `src/storage/sqlite.rs` (sqlx async-std runtime, single connection per
operation). The autosave stream loops are frequent, and the watcher also writes.

**Risk:** long autosave SQL operations can race an incoming watcher event; today
they're sequenced through SQLITE_BUSY retries but no explicit concurrency
diagnostic surfaces.

**Recommendation:**
- Pin a single connection per store and surface `SQLITE_BUSY` as a user-visible
  toast rather than silently retrying inside `cx.spawn`.
- Wrap SQLite writes in `BEGIN IMMEDIATE; …; COMMIT;` so the autosave + watcher
  serialization is explicit.

### 3.2 `watcher::WatcherHandle::spawn` clone-of-clones

**Where:** `src/native/clipboard/watcher.rs`. The watcher spawns two OS threads per
invocation (one for notify events, one for the inner poll loop). With multiple
store instances this leaks FDs. **Recommended:** a global watcher registry, per-store
listeners dispatched from a single rdev/native notify thread.

### 3.3 `panic` handler for the global allocator + UI main

The current `global_allocator = mimalloc` is fine but the app installs no panic
hook. A `set_hook` that:
1. Captures a frozen sticker snapshot (relevant only when the panic is on the
   GUI thread).
2. Writes the panic to `crash_log.txt` (already plumbed by `utils/logging.rs`).
3. Exits with `ExitCode::FAILURE`.

…would detect GPUI crashes that today produce an empty Windows app exit without a
trace.

---

## 4. Ergonomics — Design Guide followup (UX wins for cheap)

These map directly to the Design Guides (`docs/plans/clipboard-design-guides.md` and
the upstream `gpui-kit-design-guides` skill). Each item is a concrete fix.

### 4.1 Manager: keyboard path and focused-list semantics

Today:
- The search input has focus.
- The sticker cards cannot be reached by Tab.
- Mouse double-click is the only activation path.

This is the largest **ergonomic** gap. After virtualising (1.1), the manager
becomes passable. The follow-ups:
- **Tab** to enter the list → **↑/↓** to navigate → **Enter** to open, **F2** to edit,
  **Del** to remove (consistent with the clipboard chooser's shortcuts).
- Visible **lane** for hover state, selection state, focus state (per Design Guides).
- **Right-click → context menu** with `Open`, `Edit`, `Pin`, `Delete` — same actions as
  the chooser but in list scope.

### 4.2 Clipboard row actions: discoverability

The hover-revealed row action strip in `clipboard.rs` (`tooltip("Copy · Ctrl/Cmd+Enter")`
+ `tooltip("Protect · Ctrl/Cmd+L")` etc.) is fine for mouse users, but the shortcuts
aren't surfaced in the footer's hint line. The footer says `↑↓ · Enter paste · F2 edit · Esc`
— add `· Ctrl+R pin · Ctrl+L protect · Ctrl+Enter copy only` and the chooser
becomes fluent.

### 4.3 Destructive confirmations: copy + mapping

- **Delete sticker** dialog already correctly names the object and uses
  `Button::danger()` (milestone 3). Audit: did we also update clipboard `remove`?
  The unprotect → delete flow currently robs protected items without confirmation. Add
  an `AlertDialog` for `Forget protected content`.

### 4.4 Empty / loading / error states

The chooser uses three distinct empty messages (`Loading…`, `Your copied text will
appear here`, `Enable collection below to get started`, `No matching items`, `Try a
different search or clear the filter`). Manager list shows nothing during loading.
Add a skeleton row (a single dimmed card place-holder) for the duration of the
load — Design Guides call this out explicitly:

> "Switching between settings panels whose content is already available should be
>  immediate; a skeleton is appropriate only while content is genuinely loading
>  and its shape is known."

The manager knows the load is bounded by `STICKER_LOAD_LIMIT` and a count is
already on hand. A 6-row skeleton card is cheap and matches the existing card
shape.

### 4.5 Status banner positioning

`status_banner` in `main.rs` shows the error alert on failure. The current
placement uses `.px_2().bg(transparent_black().alpha(0.0))` — a bare edge above the
footer with no chrome. Per Design Guides: "Reserve the strongest elevation for the
topmost decision layer", and a footer-aligned inline alert sits in the same
elevation as the cards. Lift it into a 1-line tagged area: `Alert::error(...).small()`,
spanning the footer's full width without breaking the grid.

### 4.6 StickerColor palette semantics (carried forward)

The split into a UI-owned palette module (`src/native/theme.rs`) lands the
separation. Next:
- Document the design rule that 卡片背景 = `0x…2d2a1b` etc. are *user-selection
  tints*, not chrome — and add a screenshot of the five pairs to the design guide
  so future designers know what's allowed under "domain color".
- Offer a "high-contrast" variant override in the theme module for users who need it.

### 4.7 Sticker footer actions: legend of icons

Audio's prev/play-pause/next/play-mode and timer's play/pause/reset/restart all
display icons without labels. On first launch users have no way to discover what
they're doing. Per Design Guides: every icon-only button needs an accessible
name and a tooltip with the meaning. Tooltips already exist; verify they're
discoverable in first-run UX (e.g., a one-time coaching overlay that points to
the icons, dismissed once any of them is used).

### 4.8 File sticker error states

`file/preview.rs` has a healthy `Some(FilePreview::WebView(_))` branch but the
fallback for missing files / unsupported encodings is missing. Per Design Guides,
show a one-line actionable error (`Icon + "Can't read PNG: corrupt header"`),
avoid the current silent empty surface.

### 4.9 Sorting affordance

Manager's `sort_button` is the `IconName::SortDescending` chip. The dropdown has four
options; the active one already has its `checked` styling. **Miss:** there is no
visible sort-direction label when the menu is closed. Add a `tooltip("Sort: …")` so
users know what's current without opening the menu.

### 4.10 Tray / status surface

Rustickers has no system-tray / status-bar surface. On Windows, the app is a
collection of sticker windows with no central "I'm still alive" indicator. A
minimal tray icon (pause / resume collection toggle) closes the loop on the
"Collection paused / paused · protected history" banner rule. The Guide
explicitly suggests persistent navigation for utility apps; a tray icon meets
that goal with minimal surface area.

---

## 5. Architecture / refactor — high-payoff followups

### 5.1 Listeners-as-data tables

`button on_click`, `row on_mouse_down`, `input on_change` all do the same thing: a
closure captures `id`, builds the right thing, and dispatches via
`cx.listener`. Define one tiny helper per control family:

```rust
fn row_listener<E, F>(entity: &Entity<E>, id: i64, handler: F) -> impl Fn(&mut E, &MouseEvent, &mut Window, &mut App)
where
    F: Fn(&mut E, i64, &mut Window, &mut App) + 'static
```

Then in render:

```rust
.extend(map_row(entity, *id, |this, id, w, cx| open_at(this, id, w, cx)))
```

This pulls the `cx.listener` boilerplate out of every render path and gives the
compiler one closure shape to optimise. Touches ~30 listener sites in sticker
windows and clipboard rows.

### 5.2 StickerColor breadcrumb

The `StickerColor::bg()/swatch()` shape is still leaking into render
(`sticker.color.bg()` in paint, markdown, sticker chrome). Once they live in
`native::theme::sticker_background(...)`, harden further: a `CardSurface`
struct with `bg`, `border`, `accent` so the card render passes one bag, not
three spaced lookups.

### 5.3 `cx.new` chokepoints

A handful of sticker views have `$sticker_events_tx.clone()` chained through
several constructors. Combine them into a `StickerContext` struct captured once at
window open. Code generator: this is what 90% of stored stickers will need.

### 5.4 Pre-create lifecycle hooks

The bootstrap creates `EventEmitter<StickerWindowEvent>` and uses `mpsc::channel`
across panics. A typed bus + bounded channels avoids the
`tokio::sync::mpsc::error::SendError` runs that occasionally surface on shutdown.

---

## 6. Quick-hit tidyups (no design discussion needed)

| Site | Action |
| --- | --- |
| `src/native/components/stickers/file/mod.rs:588` | `match → !matches!(self.preview.as_ref(), Some(FilePreview::WebView { .. }))` (clippy lint) |
| every row `cx.listener` cloned on render | Move construction to `cx.entity().clone()` + `cx.listener(...)` once in `new`; pass `UpdateGlobal` global |
| `interaction_observer` never registered | The Kit has touch / drag event interception available; useful for the selection popup |
| `main.rs` `STICKER_EVENT_PUMP_INTERVAL` (120 ms) | Probably fine; profile before tuning |
| `rg '\.clone\(\)' src/native/` (453 hits) | Profile first; most clones of `StickerColor`, skinner `Entity<...>` clones, and `mpsc::Sender::clone` (which is just an Arc bump) are cheap. The expensive clones are large strings; those are the ones to address. |

---

## 7. Prioritisation

The recommendations sorted by **pay-off / cost** — start from the top.

1. **§1.1 — virtualise the manager list.** Highest performance, fits the
   accessibility table, prerequisite for keyboard path.
2. **§4.1 — manager keyboard path and context menu.** Cheap once §1.1 lands; large
   ergonomic win.
3. **§2.1 — bind timer / autosave loops to the window lifecycle.** Reliability
   fix; small patch.
4. **§4.2 / §4.5 — footer hint surface and status banner alignment.** Cheap UX
   wins; no behavior change.
5. **§2.4 — IPC improvements.** Single-instance and corpse-socket cleanup are
   recurring tickets; address them once.
6. **§5.1 / §5.2 — listener/surface refactors.** Cleanup with secondary
   performance benefits.
7. **§3 reliability — SQL contention, panic hook.** Polish; once the others
   land, run a profiling session in the field and pick what's still hot.
8. **§4.10 — system-tray surface.** Out-of-scope fluff for the kit migration but
   it's the single biggest continuous-discovery win.

Sections **not** on this list are deliberately left as future work where the
migration is intentionally incremental. Feel free to triage in whichever order
fits your availability; **§1.1 is the one I would not skip.**
