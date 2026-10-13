# Architecture

Rustickers has two entry points and one local database:

```text
rustickers (desktop) ── GPUI windows / hotkeys / clipboard observer ─┐
                                                                  ├─ stickers.db
rst (CLI) ──────────── commands / text selection / child execution ─┘
```

The desktop is single-instance. IPC carries requests from the CLI or another launch; internal window events refresh the manager. Database-change polling catches writes from other processes without opening sticker windows.

## Code map

| Location | Responsibility |
| --- | --- |
| `src/rustickers.rs`, `src/rst.rs` | Desktop and CLI startup |
| `src/ipc.rs` | Single-instance ownership and external requests |
| `src/model/` | Sticker identity, type, state, and serialized content |
| `src/storage/` | `StickerStore`, SQLite implementation, app paths |
| `src/text/` | Shared text selection, capture lifecycle, encryption |
| `src/native/windows/` | Manager, sticker windows, TXTs, selection popup, dialogs |
| `src/native/components/stickers/` | Markdown, timer, command, file, and paint views |
| `src/native/clipboard/` | OS collection, write suppression, verified paste |
| `src/cli/` | Explicit sticker commands, separate text workflows, terminal chooser, exact argv, bundled recipes/agent guide |
| `migrations/`, `tests/` | Database evolution and regression coverage |

Features: `ui` is enabled by default; `cli` builds `rst`; both depend on `storage`. UI dependencies enter through GPUI Kit and its matching WebView adapter.

## Storage and text lifecycle

`stickers.db` is the only runtime store. SQLx applies migrations on open. General sticker operations use `StickerStore`; text operations use `TextRepository` on the same SQLite pool so multi-step changes can commit atomically.

| Table | Purpose |
| --- | --- |
| `stickers` | Persistent stickers, content, window placement, text revisions/recency, plaintext digest |
| `clipboard_captures` | Exact disposable text, digest, byte size, creation/copy times |
| `text_master_key` | Public recipient and password-wrapped private identity |
| `text_imports` | One-time legacy-import markers |

Historical clipboard/vault tables remain in the migration chain for raw import only; they have no runtime writers. Import skips unusable or encrypted-only content and leaves the old database untouched. Never modify an already-applied migration: add a new one.

TXTs contains Markdown stickers and optional captures, not command output or file previews. Typed IDs (`sticker:42`, `clipboard:42`) prevent cross-table collisions. Both desktop and CLI use shared full-text filtering and deterministic MRU ordering. Locked entries expose only public titles/metadata.

Capture promotion creates a **closed Markdown sticker** and consumes the source in one transaction. Capture budgets never evict saved stickers. Content/revision checks reject stale saves, duplicate promotion, and authorization races. Custom titles survive editing; title changes do not change text-use recency. `text_digest` is computed at seal/save/promote time on the **plaintext** bytes, so dedup happens across encryption transitions: a clipboard copy whose content matches a saved (plain or locked) sticker refreshes its recency instead of materializing a new capture row, keeping duplicate plaintext from lingering beside a sealed copy. Startup persistently backfills legacy plaintext sticker digests and consumes matching disposable captures, preserving their activity on the saved sticker. General sticker writes, text edits, protection changes, and legacy import use the same reconciliation policy. Legacy locked rows gain a digest only when an authorized edit, unlock, or password rotation provides plaintext; ciphertext is never hashed as a substitute. Intentionally separate saved sticker identities are not merged.

## Protection

`src/text/protection.rs` uses `age` recipient encryption. A random X25519 identity is wrapped with an Argon2id-derived key; the password is a credential, not the private key itself.

- Encryption uses the readable public recipient; decryption authorizes one action.
- No private-key cache or global unlock deadline exists.
- Ciphertext is bound to sticker identity and key generation.
- Password change generates fresh keys and rewrites all supported ciphertext in one transaction. Any failure rolls back everything.
- Editors hold plaintext only for their scoped session. Dirty encrypted drafts block rotation; stale revisions cannot overwrite rotated content.

Legacy locks, clipboard vaults, and external-file locking are unsupported. There is no password recovery. Encryption cannot erase old backups, SQLite remnants, OS history, or other applications' copies.

## Runtime boundaries

The GUI thread owns entities, focus, and window state. Database work, command execution, and expensive crypto run outside rendering; completions update their owner through GPUI contexts.

Command stickers keep execution/scheduling state independently of window visibility, so armed cron jobs can run while closed. File previews use a fallible WebView and never lock source files in place.

Windows shell COM calls can run a nested message pump. Explorer selection and virtual-desktop queries/restores must run on the UI thread **between** GPUI updates, with no `App`, entity, or window borrow held. Capture a native handle in a short update, perform COM after it returns, then apply the result in another update. Rendering/bounds sampling reads the last successful virtual-desktop ID instead of invoking COM; query failures preserve that ID. `cx.defer` still holds `App` and is not a safe COM boundary. The wake guard locates this process's hidden top-level GPUI platform window (with a message-only fallback for older releases).

The clipboard observer writes ordinary captures through `TextRepository`. Internal decrypted writes are suppressed; externally recopied secrets are not guaranteed exclusion. Paste verifies the destination and modifier release. Windows supports ownership-aware protected clearing; unavailable capabilities fail closed. A newer clipboard value must never be cleared.

The CLI separates sticker `list` from `text list`/`text exec`. Direct command creation accepts an argument vector after `--` and round-trip-validates its encoding into the desktop's command storage; shell scripts are explicit. `{{txt}}` is substituted only in immediate child argument tokens, never the executable. It preserves exact text, inherits stdio, and propagates child exit status. Selection stickers use `{{RUSTICKERS_SELECTION}}` or the environment variable; shell selection uses environment data rather than interpolation into code. No implicit shell execution or expanded-secret argv logging is allowed.

One invocation-owned runtime carries storage location and IPC policy through ordinary commands and recipe expansion. `--data-dir` and library test execution are always offline, preventing experimental IDs from controlling real windows. Notification status describes transport delivery, not application acknowledgement. Parsing/validation precede storage initialization; help, recipes, and the bundled agent guide are database-free. JSON usage and runtime errors use the same `ok` envelope. Recipe previews/execution share one nonrecursive argv expansion; tests enforce parser and documentation synchronization.

## Making changes

Keep domain invariants in storage/text services and presentation in native views. New commands reuse shared parsing/output helpers; new sticker types require model, view, storage, and CLI integration. Test persistence against temporary databases, never live user data.

See [FRONTEND.md](FRONTEND.md) for UI rules and verification commands, and [README.md](README.md) for usage.
