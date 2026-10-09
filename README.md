# Rustickers

A lightweight desktop sticker app for notes, timers, command output, file previews, and sketches. Built with [GPUI Kit](https://crates.io/crates/gpui-kit) and local SQLite.

![Rustickers demo](./screenshots/demo1.png)

## Run

Install stable Rust, then:

```bash
cargo run --bin rustickers
cargo run --bin rst --no-default-features --features cli -- --help
```

For release binaries:

```bash
cargo build --release --bin rustickers
cargo build --release --no-default-features --features cli --bin rst
```

## Desktop shortcuts

These are the defaults. Edit and save the four global shortcuts in **Settings**; changes take effect immediately and survive restart. Use Cmd instead of Ctrl on macOS.

| Shortcut | Action |
| --- | --- |
| Ctrl+Alt+R | Show the manager |
| Ctrl+Alt+V | Open or dismiss TXTs |
| Ctrl+Alt | Toggle quick file preview |
| Ctrl/Cmd+Space | Send selected text to a command sticker |
| Esc | Dismiss a transient window |
| Ctrl+S | Save editor content |

Command stickers support scheduled runs, including while their window is closed. File previews never encrypt or overwrite the source file.

## Manager

The manager organizes stickers by **Text / Commands / Timers / Sketches / Files**, with dedicated **Scheduled** and **Selection commands** views. Search titles, command lines, folders, and cron expressions instantly. Hover a row for Open/Delete icon actions; Enter opens and Delete asks for confirmation. Command stickers open for inspection/settings without automatically rerunning.

**Settings**, at the bottom of the sidebar, brings together master-password management, clipboard collection, and editable global shortcuts. Shortcut changes are staged until **Save shortcuts**; **Use defaults** fills the defaults without applying them. The sidebar collapses in narrow windows; Ctrl/Cmd+F focuses search, and arrows/Enter navigate and open the collection.

## TXTs: saved text and clipboard history

TXTs combines Markdown stickers (open or closed) with disposable clipboard captures. Search, filter by **All / Text / Clipboard**, and navigate with arrows or Page keys. Each item shows up to three lines; hover its trailing actions to rename, edit, save, lock, or delete. TXTs closes when it loses focus.

- **Enter** pastes to the previous app when opened through the global shortcut; from the manager, it copies instead. Ctrl/Cmd+Enter always copies.
- **F2 / double-click** opens the normal sticker editor. Editing a capture first saves it as a sticker.
- **Save as sticker** (Ctrl/Cmd+R) turns a capture into a closed sticker.
- **Lock / Save encrypted** (Ctrl/Cmd+L) asks for a required public title, then encrypts without opening a sticker.
- **Rename** (Ctrl/Cmd+F2) changes a saved sticker's title; custom titles survive content edits. Locked stickers also show an editable public-title field above Unlock.
- Deleting a capture is immediate; deleting a saved sticker requires confirmation.

Clipboard collection starts **paused**. Enable it in TXTs. Captures preserve exact text and share a budget of 500 entries / 20 MiB; saved stickers are not evicted. When a new clipboard copy's content matches an already-saved text sticker (plain or locked) by digest, the saved sticker absorbs the recency and no capture row is created, keeping duplicate plaintext from lingering next to a sealed copy. Automatic paste and ownership-safe protected copying currently have a supported path only on Windows.

## Protection and privacy

Set or change the **Master password…** from the manager or TXTs locker. Once configured, encryption needs no password; each decryption requires it. There is no global unlock session. **Remove encryption…** is a separate authenticated action.

Changing the password generates new keys and re-encrypts all supported locked text atomically. **There is no forgotten-password recovery or old-lock-format compatibility.** Titles stay public and searchable: do not put secrets in them.

Internally copied decrypted text is excluded from collection, but externally copied secrets can enter history. The stored digest of an encrypted sticker lets a later copy of identical plaintext refresh the locked sticker's recency instead of adding a new plaintext capture, so the sensitive row does not silently get duplicated beside its sealed copy. Protected clipboard text is cleared after 30 seconds only if still owned; a crash may prevent clearing. Encryption does not erase OS clipboard history, other apps' copies, old database pages, backups, or the persisted digest (a digest reveals content equality and permits offline guessing of low-entropy secrets; it is omitted from redacted JSON output). Legacy locked stickers gain a digest only after an authorized edit, unlock, or password rotation; until then identical external copies cannot be recognized without decrypting. Protect backups too.

## CLI

```bash
rst ls                                  # open stickers; list is an alias
rst ls --text --cp -s notes              # merged text/clipboard search
rst ls --text --cp --json                # redacted metadata
rst ls --text --cp -- my-tool '{{txt}}'   # choose text and pass it as an argument
rst ls --text --id sticker:42 -- my-tool '{{txt}}'
rst markdown -c "Remember this"
rst view ./report.html
rst cmd "cargo test" --dir . --result text
rst cmd "npm test 2>&1 | tail -20" --shell
rst cmd "gh pr list" --cron "0 */5 * * *"
rst show 42
rst result 42
rst open 42
rst close 42
rst delete 42
```

`--text` selects text stickers; `--cp` selects captures. Combine them for both. IDs are source-specific: `sticker:42` and `clipboard:42`. Without a child command, `ls` prints a table or JSON. The old `cp` and `vault` commands are removed.

Quote `{{txt}}` for your shell. Substitution preserves text as argument data; without a placeholder, it is appended as one argument. Rustickers does not implicitly invoke a shell. Passing text to an interpreter's code argument can execute it, and secret argv can be visible to other tools. Use `rst <command> --help` for options; `rst skill list` lists ready-made sticker recipes.

## Data

The app and CLI share `stickers.db`; logs live beside it in `logs/`.

- Windows: `%LOCALAPPDATA%\rustickers\data\`
- macOS: `~/Library/Application Support/rustickers/data/`
- Linux: `~/.local/share/rustickers/data/`

Startup imports usable raw text once from legacy clipboard storage, without recovering encrypted content. The old `clipboard.db` is left untouched and may contain sensitive data. Log level: `RUSTICKERS_LOG`, then `RUST_LOG`, then the build default.

## Contributing

See [ARCHITECTURE.md](ARCHITECTURE.md) for the system map and [FRONTEND.md](FRONTEND.md) for desktop conventions and checks.
