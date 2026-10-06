# Clipboard history: product and implementation plan

**Status:** Implemented locally and under verification; cross-platform/protection release gates remain open.  
**Goal:** Automatically collect copied text and make it reusable from a clean, keyboard-first GUI and a safe interactive CLI. 所有操作和使用符合人体工程学：少按键、少切换、可预测、可撤销。

Use this document as the implementation checklist and decision record. Do not mark a task complete without the evidence required by its milestone.

## Current implementation status — October 6, 2026

The milestone/evidence notes below include historical claims from the initial implementation;
this audit supersedes them where they disagree with current code. Do not treat this plan as
proof that clipboard protection or every platform is release-ready.

- CLI chooser now uses raw controlling-terminal input, type-to-filter, arrows/Page keys,
  Enter and Esc/Ctrl+C, and restores terminal state before child launch. `--search` prefills
  interactive filtering; `--list` respects human/JSON mode. Clap owns child argv after `--`.
- Direct child execution preserves streams and status. Selection is reread after choosing
  and authorization. Raw `cp show` preserves exact text without appending a newline;
  JSON output uses the standard envelope.
- Protected import authorizes and encrypts **before** insertion. No failed-protection
  plaintext fallback. CLI authorization is process-local; standalone `vault unlock`
  verifies credentials, not a persistent desktop or cross-process session.
- Ordinary dedupe is exact (case/whitespace included); edited digests are updated.
  Insert/pruning and limit changes are transactional. Protected previews and search do
  not consult plaintext metadata; flag-only protection is refused. Vault recreation is
  refused, expiration revokes cryptographic access, and stale editor saves fail.
- Desktop history has virtualized themed rows, stable selection, explicit copy/paste,
  reservation and a single editor. Automatic paste restores/verifies the destination
  on Windows and fails closed elsewhere. Ctrl/Cmd+L authorizes an atomic, generation-bound
  ordinary-to-protected transition; encrypted editor saves use the same generation checks. Collection is opt-in, and remains paused while
  protected items exist rather than duplicating secrets as ordinary plaintext.
- Remaining acceptance work includes active-use leases, ciphertext identity/version AAD,
  historical plaintext/journal/backup analysis, Windows ownership-conditional expiry validation,
  OS exclusion validation, deletion/undo and unprotection UX, native macOS/Linux paste adapters,
  IME/accessibility/DPI checks, and measured latency/overhead. Do not mark M4/M5 complete.

## 1. Scope and decisions to confirm

### Confirmed requirements

- Automatically track copied text while the collector is running.
- A global shortcut opens searchable clipboard history; typing filters, Enter uses the selected item in the previously focused application.
- Protected items prompt for the master password before use.
- `Ctrl+L` protects an item; `Ctrl+R` reserves it; double-click edits it.
- Edited, protected, and explicitly reserved items are never evicted by the normal history limit.
- Ordinary history is bounded. GUI and CLI share filtering, retention, and sorting behavior.
- `rusticker cp ls -- some_cmd xxx {{cp}} xxx` opens a type-to-filter chooser. Enter substitutes the selected text; without `{{cp}}`, append it as one argument.
- The interface should be clean, ergonomic, and consistent with the existing desktop app.
- The exact global binding is delegated: choose a convenient, configurable shortcut, not a mandatory `Ctrl+Home+.` chord.
- `Ctrl+L` protects only the selected item, not the whole history.
- CLI command use is execution-only: stream the child's output and propagate its exit status. Do not print a generated command or substitute clipboard text as the result.

### Confirmed decisions and remaining proposed defaults

D1–D3 are confirmed by the user. D4–D9 remain proposed defaults, not approvals.

| ID | Decision | Status / consequence |
| --- | --- | --- |
| D1 | Convenient global shortcut | Confirmed: exact binding is delegated; `Ctrl+Home+.` is not required. Choose a compact-keyboard-friendly, configurable chord after checking conflicts. Candidate: Ctrl+Alt+V (Cmd+Alt+V on macOS), subject to platform verification; also expose a main-window action. |
| D2 | Item lock vs. whole-history lock | Confirmed: `Ctrl+L` protects only the selected item using the shared master password. Clearing temporary unlock state does not lock ordinary history. |
| D3 | CLI execution and output | Confirmed: with a command after `--`, Enter substitutes/appends the selected text, executes directly, streams child stdout/stderr, and propagates the child exit status. No print-command mode or parent-shell prompt insertion. |
| D4 | Normal history limit | 500 ordinary items, configurable. Add a 20 MiB ordinary-text budget and a 1 MiB per-capture limit as initial safeguards, subject to measurement. Oversized captures are skipped with a visible, non-modal explanation, never truncated silently. |
| D5 | Ordering | One shared MRU order for all items; reserved/protected/edited items receive badges, not a separate sort tier. Successful reuse and a new copy promote an item. |
| D6 | Unlock lifetime | One-shot unlock for paste/CLI execution; temporary unlock in the editor, cleared on close or inactivity. No background sharing of an unlocked master key with other processes. |
| D7 | Collection lifecycle | Desktop app owns collection while running, including when its main window is closed. CLI can query stored history without the app. Headless `cp watch` is a later phase, not an implicit daemon started by `cp ls`. |
| D8 | First-run privacy | Explain that copied secrets can enter ordinary history; require explicit enablement once, then collect automatically. Offer pause and clear controls. |
| D9 | Retention after removing protection | Remember each retention reason independently. Unprotecting an item does not automatically make it disposable; releasing saved retention requires an explicit action. |

**Confirmation gate:** D1–D3 are settled. Resolve the initial privacy/locking model and remaining proposed defaults before implementation. The UX below is a provisional interaction brief, not a finalized visual design. Before building the GUI, confirm its layout against the current app and capture the agreed visual direction.

### Non-goals for the first release

Images, rich clipboard formats, file-copy history, cloud sync, OCR, AI categorization, password recovery, browser extensions, command rendering/parent-shell prompt insertion, and arbitrary shell evaluation. Capturing every intermediate copy cannot be promised when an OS provides only the latest clipboard value; document platform limitations honestly.

## 2. Shared behavior contract

### 2.1 Capture and identity

1. Watch OS clipboard changes rather than keypresses: collect mouse-menu copies, programmatic copies, and text from any application where the OS exposes it.
2. Preserve text exactly: whitespace, Unicode, line endings, and multiline content. Ignore empty text, not whitespace-only text. Normalize only search keys and previews.
3. Deduplicate ordinary entries by exact text. A repeated copy updates `last_copied_at` and recency instead of creating another row. Confirm content equality, not just a hash match.
4. Keep IDs stable across filtering, promotion, editing, and reservation. Edit in place; do not merge saved/protected records simply because their contents match.
5. Once edited, an item is user-owned. An incoming copy must not overwrite its content or retention reasons; a matching copy may promote it without changing it.
6. Start by capturing the current clipboard once after enablement. Pause means no capture and no backlog; resume reads the then-current value once.
7. Retry a busy clipboard with bounded backoff. Surface persistent failure or missing permission in status/settings without stealing focus.
8. Suppress Rustickers-owned clipboard transactions: selection-capture markers/restores, history paste writes, and protected-copy writes. Serialize internal write/capture transactions with the collector, using native generation/ownership data where available—not text matching or a broad time window alone.
9. Existing selected-text command capture must not generate synthetic history rows. Decide explicitly whether its genuine captured selection is ingested once; proposed default: yes, unless protected or excluded.

### 2.2 Retention and eviction

Keep independent, durable retention reasons:

- `explicit`: user pressed Reserve.
- `edited`: a changed edit was successfully saved (opening/canceling/no-op saving does not count).
- `protected`: the item was successfully password-protected.

```text
retained = explicit || edited || protected
ordinary = !retained
```

A protected envelope is always retained, even if metadata is damaged. Unprotecting clears encryption, not the historical `protected` retention reason. “Make ordinary” explicitly clears retained reasons only after protection is removed and the user confirms the consequence.

- Ordinary item count and ordinary bytes are bounded; retained items do not consume either allowance.
- On ingest, trim the least recently active ordinary items in the same transaction. Lowering limits also trims ordinary items only, after showing the impact.
- Use the same recency key as the displayed ordering; never evict a retained row to satisfy an ordinary limit.
- Reserving/protecting/saving edits is atomic with its retention change. Concurrent capture cannot evict an item halfway through saving it.
- Hold a temporary eviction lease on an item being edited, unlocked, or used; this lease is not a permanent retention reason. Release it on success, cancel, timeout, or process exit. If no ordinary victim is available, skip the incoming capture rather than deleting active/saved data.
- `Ctrl+R` toggles the explicit reason only. If an item is also edited/protected, removing explicit reservation still leaves it retained; explain this in the badge tooltip/status message.
- Retained history is not silently capped. Show retained storage usage and warnings, plus explicit cleanup/export actions later. Disk-full errors must leave existing data intact.
- Delete is always explicit, with undo for ordinary deletions and confirmation for saved/protected deletions. Automatic eviction is not undoable by retaining a hidden, unbounded archive.

### 2.3 Ordering and search

Shared default ordering:

```text
recency_at DESC, created_at DESC, id DESC
recency_at = max(created_at, last_copied_at, last_used_at, last_edited_at)
```

Reserve/protect/unlock alone do not reorder a row. Apply promotion after an interaction completes, not while the user is choosing. Keep the highlighted ID stable when new captures arrive; if it disappears, select the nearest surviving row. Search changes select the first match.

- Case-insensitive Unicode-aware token matching: all query tokens must match. No fuzzy score reordering in v1; filtered results retain the exact same MRU ordering in GUI and CLI.
- Search ordinary/unprotected content and optional user labels. Protect against stale asynchronous query results.
- Protected rows expose only a generic “Protected item” label or an explicitly public user alias. Never persist/index a plaintext preview, title derived from content, or plaintext content hash for a protected item.
- Locked protected content is not searchable. Both surfaces say so; reserved/protected-only filters allow discovery without leaking text.
- Preview text is shortened and escapes control characters; the stored text and pasted/argument payload remain unchanged. Terminal escape sequences from clipboard content must never execute in the chooser.

## 3. GUI interaction brief

**Job:** retrieve a past copy without losing the current task or destination. **Mode:** Operate. The primary action is paste; management actions stay secondary and discoverable.

### Main flow

1. Global shortcut snapshots the foreground destination **before** opening any Rustickers window.
2. Open one transient history window near the active display's center, constrained to its work area. Repeating the shortcut dismisses it; it must not replace the saved destination with Rustickers itself.
3. Search field receives focus, first row is selected. Type → filter; arrows → select; Enter → use.
4. For a protected row, show an inline master-password prompt. Preserve query, selection, and destination. Enter submits; wrong password stays in place with an error; Esc cancels the prompt back to the chooser.
5. On successful authorization, hide the chooser, wait until shortcut modifiers are released, restore and verify the original destination, write plain text, and synthesize the platform paste action.
6. If the destination has disappeared, is elevated/inaccessible, or focus restoration fails, do not paste into an arbitrary window. Offer “Copy only” and let the user paste manually.
7. Record `last_used_at` only after successful clipboard/paste dispatch. OS dispatch is not proof the destination accepted the paste; do not claim guaranteed delivery.

Do not immediately restore the previous clipboard after paste: applications may read it asynchronously. Normal selection remains on the clipboard. Protected text receives best-effort timed clearing, conditional on its ownership/generation being unchanged, and a warning that OS history, other managers, and destination apps may retain it.

### Keyboard and mouse contract

| Input | Chooser behavior |
| --- | --- |
| Configurable global shortcut | Open/dismiss history; convenient default chosen after conflict checks; also available from the main window |
| Printable text / IME input | Filter without moving focus away from search |
| Up / Down | Move selected row; never interpret IME candidate navigation as list navigation |
| Page Up / Page Down | Move by visible page |
| Enter | Paste selected item; request password if required |
| Ctrl+Enter | Copy only; do not synthesize paste |
| Ctrl+R | Toggle explicit reservation; announce retained reasons |
| Ctrl+L | Protect selected item; configure master password if absent; already protected → explain status |
| F2 / double-click | Edit selected item, unlocking first if protected |
| Delete | Delete selected row, with safety/undo as above; text editing retains its normal Delete behavior |
| Esc | Cancel the innermost prompt/editor, then dismiss chooser; never accidentally send content |
| Tab / Shift+Tab | Move through visible controls with a clear focus indicator |

Primary-modifier equivalents on macOS must be documented. `Ctrl+L`, `Ctrl+R`, and Delete are chooser-scoped, never global grabs; the editor preserves standard text-editing shortcuts. In the multiline editor, Enter inserts a newline, Ctrl/Cmd+S saves and returns to history, and Esc cancels (confirm only if dirty).

Single-click selects only. **Do not paste on single-click**, because double-click must edit. Add a secondary Paste button/context action for mouse-only users. Keyboard paste normally takes shortcut → optional typing → Enter; no confirmation for ordinary reuse.

### Clean UI and accessibility requirements

- Reuse GPUI, existing theme, inputs, icons, focus/dismissal behavior, and transient-window patterns; clipboard history is not a new sticker type.
- Provisional hierarchy: search header → compact virtualized list → small contextual shortcut/status footer. Optional expanded text preview on demand; no dashboard, card grid, or always-open management sidebar.
- Each row: short readable text preview, subdued recency, distinct selected state, and small labeled/status badges for Edited, Reserved, Protected. Avoid redundant icons and color-only distinctions.
- Support both existing light/dark themes where available, display scaling, keyboard focus, text contrast, screen-reader labels, and reduced motion. Do not hide necessary actions exclusively behind hover.
- Fit small screens and high-DPI displays; long URLs, CJK, emoji, tabs, and multiline clips must not break row height or horizontal layout.
- Empty history: explain collection and enable/pause state. No matches: preserve the query and offer Clear filter. Include loading, paused, permission-denied, storage-full, deleted-during-selection, unlock-error, and paste-failed states.
- Announce reservation/save errors without modal interruption. Never show a “Saved” badge until persistence succeeds.

## 4. CLI contract

### Interactive use

```bash
rusticker cp ls                                  # browse history only; no raw-text output or execution
rusticker cp ls -- some_cmd xxx '{{cp}}' xxx      # choose, replace argument placeholder, execute
rusticker cp ls -- some_cmd xxx                  # choose, append as one argument, execute
rusticker cp ls -- rg --fixed-strings '{{cp}}' .  # child flags belong to the child after --
rusticker cp ls -- some_cmd 'prefix={{cp}}'       # embedded placeholder stays in one argument
```

Quotes around the placeholder protect it from the invoking shell; they are not added to the text passed to the program. Supply shell-specific examples in final help (POSIX shells, PowerShell, Windows cmd).

- Type to filter, arrows/Page keys to select, Enter to confirm, Esc/Ctrl+C to cancel. Use the same shared ordering and retention badges as GUI.
- Use the controlling terminal for chooser/password I/O, including when stdin/stdout are redirected. Keep stdout free for child output only; restore terminal state on every exit path.
- Protected items prompt with hidden input; never accept a master password as an argument or environment variable. Cancel performs no execution or promotion.
- Validate the command/placeholder position before opening the chooser where possible. Re-read/lease the chosen row before execution to handle concurrent deletion/editing.
- With a child command, restore terminal state before launch, inherit/forward its stdin, and stream stdout/stderr directly without capturing, reformatting, or adding success banners. Propagate its exit code. Cancel returns 130; operational errors return a documented nonzero code with no execution.
- Proposed browse-only behavior: without a trailing command, show history but emit no raw selected text and execute nothing; explain that `-- <program> <args>...` is required for command use. Do not invent a default executable.
- `cp ls` must not create a command sticker or require a GUI build. Without a running collector, it reads the persisted history and explains that new copies are not being collected.

### Argument substitution and “proper quoting”

Use the argument vector already parsed by the invoking shell. **Never join it into a string, re-split it, or evaluate selected text as shell syntax.**

1. The first trailing token is a fixed executable; reject `{{cp}}` in that token.
2. Replace every `{{cp}}` occurrence in argument tokens with the exact selected text, without recursive replacement.
3. If no argument contains the placeholder, append the text as exactly one argument.
4. Launch with `std::process::Command`. Quotes are not part of argv; the platform launcher handles transport escaping. Spaces, quotes, newlines, `$()`, `&`, and `;` remain data.
5. NUL cannot be represented in a process argument: reject it with a clear error, not silent truncation. Reject excessive platform argument lengths with a helpful suggestion; a future explicit stdin mode can support large payloads.
6. Quoting does not prevent the child's own option interpretation. Recommend fixed positional placeholders and child-supported `--` terminators for untrusted text beginning with `-`.
7. A user can explicitly run `sh`, `pwsh`, or another interpreter, but substitution into its code argument can execute clipboard content. Warn/document this; v1 must not advertise shell-code interpolation as safe.

There is no command-rendering mode: do not implement `--print-command`, print expanded command lines, or insert them into the parent shell. Show only child output during execution. A child may itself print selected secrets; authentication authorizes passing the text to that program, not a guarantee that the program keeps it private.

### Non-interactive and management surface (proposed)

| Command / option | Contract |
| --- | --- |
| `cp ls --list [-s <query>] [-n <limit>] [--json]` | Non-interactive listing; protected rows expose metadata only |
| `cp ls --id <id> -- <program> <args>...` | Deterministic command execution without a chooser; protected rows still require terminal authentication; no raw-text output mode |
| `cp reserve <id>` / `cp unreserve <id>` | Change explicit reservation only |
| `cp edit <id>` | Edit via a documented editor contract; protected editing never writes plaintext temporary files by default |
| `cp protect <id>` / `cp unprotect <id>` | Require terminal authentication; keep saved retention on unprotect |
| `cp delete <id>` | Confirm saved/protected removal; explicit force flag for automation |
| `cp status` / `cp pause` / `cp resume` | Show collector/storage state; pause/resume requires an active collector |
| `cp config` | Set normal limits and collection preferences |

This table is a follow-up surface, not a requirement to implement every command in the first slice. Final argument spelling should be checked with Clap and existing CLI conventions.

Non-TTY invocation without `--list` or `--id` fails with an actionable error, never hangs. `--json` implies non-interactive listing when no child command is supplied; reject combining it with child execution to preserve the existing “one JSON object on stdout” convention. Do not expose protected text in JSON listings or incidental diagnostics.

## 5. Password and privacy design

“Locked” must mean encryption at rest, not just a hidden preview.

- One clipboard master password, separate from existing per-sticker passwords. Do not silently migrate/change sticker locks.
- Use a random vault data key wrapped by a password-derived key. Version the KDF parameters and envelope. Choose/review a memory-hard KDF such as Argon2id, or document a reviewed alternative; benchmark interactive unlock on target hardware off the UI thread.
- Encrypt each protected payload with authenticated encryption and a fresh nonce. Bind item identity/envelope version as associated data. Password changes rewrap the data key atomically rather than rewriting every item.
- Protection must replace plaintext and remove derived previews, search records, and dedupe keys in one transaction. Editing a protected item re-encrypts it; unlocking never persists a plaintext copy.
- Zeroize owned password/key/plaintext buffers where practical, clear GUI/password states, and do not log text, passwords, decrypted previews, expanded secret commands, or clipboard-derived error payloads. Ordinary clipboard content must not appear in logs either.
- The current clipboard, OS clipboard history, other managers, backups, SQLite journal/WAL/free pages, crash dumps, and target processes are outside a simple row-encryption guarantee. Review secure deletion, journal settings, vacuum/backup behavior, and residual plaintext before claiming protection for previously ordinary rows. SQLite row encryption alone does not erase old bytes.
- On a fresh later copy of the same protected secret, avoid creating an ordinary duplicate. Proposed approach: use a reviewed keyed fingerprint, with its key available only during authorized unlock; while no safe protected-match check is available, pause automatic collection if protected items exist. Make this state obvious. Decide whether this privacy tradeoff is acceptable before shipping protection; do not invent a persistent plaintext hash or claim deduplication guarantees while locked.
- Avoid heuristic “secret detection” promises. Honor explicit platform sensitive/exclusion hints where available; provide pause/exclude controls and clearly communicate their limitations.
- Unlock failures use bounded backoff without destroying data. Corrupt envelopes and wrong passwords both preserve the record. A forgotten password cannot recover protected content; resetting requires explicit confirmation and must not delete ordinary history.
- GUI and CLI independently authenticate. Do not add a plaintext/key-export IPC endpoint. If a future background vault is introduced, specify authorization and unlock lifetime separately.

**Security release gate:** document the threat model and pass residual-plaintext, recapture, logging, terminal-output, password-change, and concurrent-access tests before exposing “Protect” to users.

## 6. Architecture and integration map

Verified starting points (source takes precedence over older architecture summaries):

| Existing path | Reuse / required work |
| --- | --- |
| `src/rustickers.rs`, `src/native/mod.rs` | App startup and event pump; start/stop collector without blocking GPUI |
| `src/native/hotkey.rs`, `src/ipc.rs` | Existing platform hotkeys and action dispatch; add the chosen convenient shortcut, remapping/conflict behavior, and clipboard chooser event |
| `src/native/windows/selection.rs`, `transient_topmost.rs`, `windows/mod.rs` | Searchable selection popup, activation and Esc conventions; adapt patterns, not sticker assumptions |
| `src/native/selection.rs` | Currently writes a marker, simulates Copy, and restores previous text; add shared internal clipboard-transaction suppression before collection starts |
| `src/native/components/stickers/content_lock.rs` | Existing UI-coupled per-sticker PBKDF2/authenticated encryption; useful precedent, not a shared-master vault implementation |
| `src/storage/sqlite.rs`, `src/storage/paths.rs`, `migrations/` | SQLx store and data paths; new clipboard tables/migration, transactions, indexes, contention tests. Last current migration is `0009_add_sticker_placements.sql`; choose the next free number when implementing. |
| `src/cli/mod.rs`, `src/cli/commands/`, `runtime.rs`, `output.rs` | Register `cp`, preserve file-path shorthand and human/JSON contracts |
| `src/native/components/stickers/command/runner.rs`, `src/cli/shell.rs` | Existing argument substitution/launch precedent; do not import GPUI/native command-sticker code into the CLI-only path |
| `Cargo.toml`, `src/lib.rs` | Keep shared history/vault logic GUI-independent. `arboard`, `ring`, and `zeroize` are currently UI-only; review feature wiring and new terminal/KDF dependencies. |

### Proposed deep modules

- `src/clipboard/`: shared history interface owns identity, queries, retention, promotion, editing/protection transitions, and transaction rules. GUI/CLI must not independently calculate eviction or sort order.
- `src/clipboard/vault.rs`: versioned encryption, password setup/change, and scoped unlock interface, without GPUI or terminal dependencies.
- `src/clipboard/command.rs`: pure argv transformation plus direct process launch and child stream/exit-status forwarding. No command-rendering path.
- `src/storage/clipboard.rs`: SQLite implementation behind the history interface; avoid expanding `StickerStore` with unrelated clipboard methods.
- `src/native/clipboard/`: real platform adapters for monitoring, internal-write suppression, destination snapshot/focus restoration, and paste.
- `src/native/windows/clipboard.rs`: chooser/editor views and interaction states only.
- `src/cli/commands/cp.rs`: Clap definitions, terminal chooser/password flow, output, and process exit handling.

Inject a clock and clipboard/destination adapters at actual seams. Keep ranking, retention, and argv transformation deterministic so tests exercise the same interface the GUI and CLI use. Add abstractions for concrete platform/test variations, not one trait per helper.

### Logical persistence model

`clipboard_items`: stable ID; ordinary text **or** protected envelope (mutually exclusive); optional public alias; retention-reason flags; creation/copy/use/edit timestamps; byte size; privacy-safe dedupe metadata only where allowed. Index recency and ordinary-retention predicates.

`clipboard_settings`: normal count/byte limits, capture enablement/preferences, schema version. `clipboard_vault`: wrapped data key and versioned KDF parameters. Protection writes must never leak plaintext into sticker tables.

Prefer the existing app data directory; decide whether clipboard tables share `stickers.db` or use a separate `clipboard.db` after the SQLite residual-plaintext review. A separate database may simplify security/maintenance without rewriting unrelated stickers. Document the decision before migrations.

Only one collector owns clipboard watching per user session. Desktop and a later `cp watch` must coordinate ownership; ordinary `cp ls` never starts a second watcher. Mutations from GUI/CLI are transactional with bounded busy retries. Query results carry stable IDs; clients revalidate stale selections.

### Platform delivery

- **Windows first:** clipboard update notifications, foreground HWND/process snapshot, verified focus restore, paste via native input; test non-elevated → elevated failure, compact keyboards, non-US layouts, rapid copies, and multi-monitor DPI.
- **macOS:** pasteboard change tracking; accessibility/input permissions and Cmd paste; validate focus restoration, Spaces, and a convenient shortcut on compact/non-US keyboards.
- **Linux:** separate X11 and Wayland capability checks. Wayland may disallow background clipboard access, global shortcuts, and synthetic paste without compositor/portal support. Provide explicit copy-only/manual invocation fallback; do not claim parity without evidence.

## 7. Milestones and tracking

All implementation tasks start unchecked. Deliver vertical slices; each milestone should leave a usable, testable state.

### M0 — Confirm contracts and technical feasibility

- [x] Inspect README, manifest, hotkeys, selection capture, storage, CLI, and existing locking implementation.
- [x] Draft shared semantics, UX/CLI contracts, risks, and implementation sequence.
- [x] Confirm D1–D3: convenient shortcut delegated, selected-item protection only, CLI executes and streams child output only.
- [ ] Approve retention, privacy, capture lifecycle, unlock defaults, and browse-only CLI behavior.
- [ ] Choose/test a convenient configurable shortcut; spike native capture, destination restoration, and paste on Windows.
- [ ] Resolve secure protected-recapture policy, cryptographic design, and shared vs. separate database.
- [ ] Confirm GUI interaction/layout with real clipboard samples and existing app screenshots; record the approved direction.

**Exit evidence:** approved decisions recorded here; platform capability notes; no unresolved security model hidden behind a UI lock icon.

### M1 — Shared history + persistent bounded capture

- [x] Add GUI-independent history model/interface and migration(s).
- [x] Implement exact ordinary dedupe, shared ordering/filtering, independent retention flags, and transactional pruning.
- [ ] Add capture lifecycle/status and internal-write suppression, including current selection capture.
- [ ] Implement Windows watcher; persist across app restarts; pause/resume; busy/storage/oversize handling.
- [x] Add non-interactive `cp ls --list [--json]` as an inspection surface.

**Evidence (M1):** `src/clipboard/{items,filter,ordering,retention,settings,storage}.rs`; migration `migrations/0010_add_clipboard_history.sql`; 19 unit tests in `src/clipboard/*`, 5 SQLite integration tests in `tests/clipboard_storage.rs`. The CLI-only build remains GUI-free (`cargo check --bin rusticker --no-default-features --features cli` succeeds). Capture watcher, internal-write suppression, and IPC hand-off are intentionally deferred to M3.

### M2 — CLI chooser and safe command use

- [x] Register `cp ls`, terminal chooser, query/selection, terminal cleanup, and non-TTY behavior.
- [x] Implement argv placeholder/append logic with fixed executable validation.
- [x] Execute directly, preserve child streams/exit status, and update shared MRU.
- [x] Define/test cancellation, stale row, child launch failure, JSON combinations, and protected-row metadata-only behavior.
- [x] Verify execution streams child stdout/stderr unchanged; no generated command or raw selected-text output, including browse-only invocation.

**Evidence (M2):** `src/cli/commands/cp.rs` + 5 unit tests in the same file; arrow-key navigation is not yet implemented (line-driven chooser, see `choose_interactively`); raw-mode upgrade can land with M5. Exit codes: 130 on cancellation, 127 on missing program, 1 on empty history. `--list` (with `--search` / `--limit`) and `--id` give non-interactive entry points. Protected rows refuse to be substituted into a child command (caller must unprotect through the GUI first).

### M3 — GUI retrieve, reserve, and edit (mostly complete)

- [x] Implement and ship the chooser floating window with type-to-filter, ↑/↓/PgUp/PgDn nav, Esc dismiss, Enter paste, Ctrl+L toggle protection, Ctrl+R row-editor, and Ctrl+Backspace delete. (`src/native/windows/clipboard.rs`)
- [x] Wire the global `Ctrl+Alt+V` hotkey to the chooser, route the focused HWND through IPC, and close the chooser on Escape via `set_clipboard_escape_target`. (`src/native/hotkey.rs`, `src/native/mod.rs`, `src/ipc.rs`)
- [x] Implement foreground restore + rdev-synthesised Ctrl/Cmd+V paste helper. (`src/native/clipboard/paste.rs`)
- [ ] Five-second soft-delete undo banner (`delete_selected` in `src/native/windows/clipboard.rs` deletes immediately; needs banner + timer).
- [ ] Optional row-metadata dialog polish (F2 alias for Ctrl+R, label auto-complete against recent labels).

**Exit evidence (so far):** `Ctrl+Alt+V` (and `Cmd+Alt+V` on macOS) appears as an enum variant in
[`src/native/hotkey.rs`](../src/native/hotkey.rs) with two new smoke tests, the IPC contract in
[`src/ipc.rs`](../src/ipc.rs) carries `ToggleClipboardChooser { focused_window }`, and the
single-instance chooser [`src/native/windows/clipboard.rs`](../src/native/windows/clipboard.rs) is
opened with that focus handle and dismissed by global Esc via
[`crate::native::windows::set_clipboard_escape_target`](../src/native/windows/mod.rs). The
captured row id is recorded through `mark_used` and the selected content is delivered via
`write_clipboard_text` so the watcher does not re-ingest it. Double-click pastes; Enter pastes;
Esc closes; up/down navigate.

**Pending entries:** double-click conflict with reserved-row popup, F2 editor dialog, F1/Ctrl+R
row-level metadata view, deterministic DPI/IME regression, and a real cross-platform paste
helper (currently the chooser delegates the cross-platform paste via the existing
[`selection::capture_selection`] helper but does not yet call `SimulateCtrlV` directly).

### M4 — Master-password protection (not release-complete)

- [ ] Implement password setup/confirmation, vault envelopes, password change, and scoped authentication.
- [ ] Add `Ctrl+L` (selected-row protection), inline GUI unlock-and-continue (VaultModal in `src/native/windows/clipboard.rs`), protected editing flag toggling, and hidden CLI password prompt (`cp vault create|unlock|lock|status` plus `cp import --protect`).
- [ ] Implement session/key clearing (5-minute unlocked window via `DEFAULT_ACTIVE_LIFETIME_MS` in `src/clipboard/vault_controller.rs`), protected clipboard expiry, recapture privacy policy (`protected_match_token` is set only when the vault is unlocked), and exclusion hints where supported.
- [ ] Verify metadata redaction, residual plaintext handling (Zeroizing buffers in `src/clipboard/vault.rs`), logging (redacted `Debug` for `VaultEnvelope` and `UnlockedVault`), backups/journals, and concurrent protection/capture behavior (Arc + std::sync::Mutex owned by `VaultController`).

**Required exit evidence (not yet demonstrated):** protection meets the written threat model; wrong password/cancel/corruption never executes or destroys data; protected content is absent from plaintext database/search/log artifacts covered by that model; unprotecting does not silently drop retention.

### M5 — Cross-platform hardening and release

- [ ] Implement/validate macOS adapter and permissions; define Linux X11/Wayland support honestly.
- [ ] Add management commands and settings needed for maintenance; optional headless watcher only with single-owner coordination.
- [ ] Benchmark and fix capture overhead, filtering, virtualized rendering, unlock responsiveness, and contention.
- [ ] Update README/help/architecture with shipped behavior, shortcuts, limitations, security guidance, and examples.
- [ ] Complete manual platform/accessibility checks and regression checks for existing sticker/selection shortcuts.

**Exit evidence:** platform matrix, test commands/results, measured budgets, release notes, and no unchecked critical acceptance criteria for a claimed platform.

## 8. Acceptance and verification matrix

| Area | Required scenarios |
| --- | --- |
| Capture | Mouse/programmatic copy; rapid updates; repeated exact text; empty vs. whitespace; Unicode/CJK/emoji; pause/restart; busy clipboard; exclusion hints; oversized text |
| Internal writes | Marker/copy/restore transaction; clipboard-history paste; protected-copy expiry; concurrent external write during restore/clear; no accidental suppression of unrelated copies |
| Retention | Count and byte overflow; lower limits; each retention reason separately/in combination; no-op/canceled edit; unreserve edited/protected row; unprotect; explicit release; active-use leases; disk full |
| Ordering/search | Same fixture produces identical GUI/CLI IDs/order; deterministic ties; MRU on copy/use/edit; stable selection while ingesting; multilingual tokens; locked text never searched |
| Editing | Multiline save/cancel; double-click selects/edit only; edit/save versus concurrent prune/delete; protected edit remains encrypted |
| Focus/paste | Original app restored; held modifiers released; popup reopen/dismiss; nested password/editor cancel; closed destination; elevated target; permissions denied; clipboard write/input failure |
| CLI safety | Multiple/embedded/no placeholders; text containing spaces, quotes, tabs, newlines, shell metacharacters, ANSI escapes, `{{cp}}`, leading `-`; executable placeholder rejection; NUL/length error |
| CLI operation | No TTY; redirected stdin/stdout; hidden password; Ctrl+C/Esc cleanup; child not found; child exit code/signals; streaming stdout/stderr unchanged; no generated command/raw selected-text output; browse-only invocation emits no payload; JSON success/error convention |
| Protection | First setup; mismatch/wrong password; nonce/envelope corruption; password change failure; scoped relock; logs/redaction; SQLite residuals; protected recapture; clipboard expiry ownership |
| Compatibility | Existing main/preview/selection shortcuts and Esc; sticker migrations and locks; UI-only, CLI-only, combined feature builds; old data with new schema |

Suggested verification commands once implementation exists:

```bash
cargo test --no-default-features --features cli
cargo test --features cli
cargo check --bin rusticker --no-default-features --features cli
cargo check --bin rustickers
cargo run --bin rusticker --no-default-features --features cli -- cp --help
```

Add focused integration tests under `tests/` for storage transactions, GUI/CLI query parity, and actual child argv transport on Windows and Unix. Native clipboard/focus/permission checks require dedicated manual runs; unit tests alone cannot prove them.

**Provisional performance targets:** visible chooser within 150 ms when warm; filtering under 50 ms for 500 ordinary + 10,000 retained metadata rows on a named baseline machine; UI stays responsive during capture/KDF work; idle watcher event-driven where possible. Report measurements rather than treating these numbers as established capabilities.

## 9. Decision and evidence log

Append decisions, measurements, and test outcomes here as work progresses. Keep unchecked tasks visible rather than rewriting the plan to imply completion.

| Date / milestone | Decision or evidence | Files / tests / follow-up |
| --- | --- | --- |
| Draft / M0 | Existing history feature is absent. Selection capture mutates/restores the clipboard; locking implementation is UI-coupled and per-sticker; shared CLI/GUI contracts need new clipboard modules. | See integration map. Initial draft recorded D1–D9 as proposed. |
| User confirmation / M0 | D1: exact shortcut delegated, prioritize convenient access. D2: protect selected item only. D3: execute only and show the command's output; remove command-rendering/raw-selection output modes. | Contracts and milestones updated. D4–D9 and browse-only behavior remain proposed; shortcut candidate still needs conflict verification. |
| M1 done | Shared history + bounded capture. 19 unit tests + 5 SQLite integration tests cover ordering, retention, dedupe, settings. | `src/clipboard/{items,ordering,retention,filter,settings,storage,argv}.rs`, `migrations/0010_*.sql`, `tests/clipboard_storage.rs`. |
| M2 done | CLI `cp ls` chooser, --list/--id/--search, safe argv substitution with placeholders, child stream + exit-code propagation, browse-only hint. 5 cp unit tests. | `src/cli/commands/cp.rs`, `src/cli/mod.rs`, `src/cli/commands/mod.rs`. |
| M3 in progress | Watcher + capture coordinator + internal-write suppression added; selection helper forwards through the same filter. Visual chooser window, hotkey binding, and editor dialog remain their own todo since they require `gpui` window plumbing. | `src/native/clipboard/{watcher,mod}.rs`, `src/native/selection.rs`, `src/native/clipboard_chooser.rs`. |
| M4 in progress | Argon2id + AES-256-GCM vault with envelope serialization, rewrap, and round-trip / wrong-password / unknown-prefix tests. Master-password prompt plumbing, protected-row envelopes, and protected recapture policy not yet wired. | `src/clipboard/vault.rs`, `Cargo.toml` (`argon2`, `rand`, `zeroize`), 5 vault unit tests. |

### Release definition of done

- [ ] All confirmed user requirements work on each claimed platform, or explicit limitations are shown in the interface and docs.
- [ ] GUI and CLI use one history behavior contract; no duplicated sorting/retention implementation.
- [ ] Ordinary limits are enforced without silent loss of edited/protected/reserved items.
- [ ] Password protection meets a documented threat model; no misleading security claims.
- [ ] Clipboard content is never implicitly interpreted as shell code.
- [ ] Everyday retrieval stays keyboard-first and predictable; destructive/exceptional operations remain recoverable or explicitly confirmed.

### Local refactor verification — October 6, 2026

- `cargo test --no-default-features --features cli`: 97 library tests passed,
  17 clipboard-storage integration tests passed, 1 placement integration test passed.
  One terminal-only smoke test is ignored in ordinary runs.
- `cargo test --no-default-features --features storage --lib`: 35 tests passed
  before the final command-length/storage-generation additions.
- `cargo check --features cli`: passed after final integration (GUI + CLI).
- `cargo check`: UI-only passed; the final combined check covers the later GUI edits.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- Terminal-only chooser smoke: passed in an `oly` PTY using three fixture rows;
  Down/Enter selected row 2 and restored the terminal. Windows UTF-8 output was
  verified with CJK text and Unicode markers after correcting the console code page.
- Exact multiline/Unicode/quote/punctuation child argument delivery and child exit
  status are regression-tested with a fixture PowerShell script, not interpolated code.
- A no-feature `cargo check --no-default-features` still fails on pre-existing
  unconditional sticker SQLite/chrono imports. The supported CLI, storage, and GUI
  configurations above are distinct from that minimal unsupported configuration.
- No real GUI screenshot, elevated-destination paste, macOS/Linux native paste,
  screen-reader/IME/DPI audit, or performance benchmark was completed. Passing
  compilation and storage tests is not evidence that these acceptance gates passed.
