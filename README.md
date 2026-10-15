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
| Ctrl+Shift+R | Show the manager |
| Ctrl+Shift+V | Open or dismiss TXTs |
| Ctrl+Alt | Preview selected files in Explorer or on the Windows desktop |
| Ctrl+Shift+Space (Cmd on macOS) | Send selected text to a command sticker |
| Esc | Dismiss a transient window |
| Ctrl+S | Save editor content |

On Windows, the selection shortcut reads selected text directly through accessibility when supported, without changing the clipboard. Recognized terminal windows never receive a blind Ctrl+C that could interrupt a running CLI; if their selection cannot be read, the dialog asks for manual input. Other apps retain clipboard-based capture as a fallback.

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
- **Edit title** (Ctrl/Cmd+U; Ctrl/Cmd+F2 also works) opens a title field. Enter saves the title and converts a clipboard entry to a text sticker; existing stickers keep their content and protection. Esc cancels. Custom titles survive content edits. Locked stickers also show an editable public-title field above Unlock.
- Deleting a capture is immediate; deleting a saved sticker requires confirmation.

Clipboard collection starts **paused**. Enable it in TXTs. Captures preserve exact text and share a budget of 500 entries / 20 MiB; saved stickers are not evicted. When a new clipboard copy's content matches an already-saved text sticker (plain or locked) by digest, the saved sticker absorbs the recency and no capture row is created, keeping duplicate plaintext from lingering next to a sealed copy. Automatic paste and ownership-safe protected copying currently have a supported path only on Windows.

## Protection and privacy

Set or change the **Master password…** from the manager or TXTs locker. Once configured, encryption needs no password; each decryption requires it. There is no global unlock session. **Remove encryption…** is a separate authenticated action.

Changing the password generates new keys and re-encrypts all supported locked text atomically. **There is no forgotten-password recovery or old-lock-format compatibility.** Titles stay public and searchable: do not put secrets in them.

Internally copied decrypted text is excluded from collection, but externally copied secrets can enter history. The stored digest of an encrypted sticker lets a later copy of identical plaintext refresh the locked sticker's recency instead of adding a new plaintext capture, so the sensitive row does not silently get duplicated beside its sealed copy. Protected clipboard text is cleared after 30 seconds only if still owned; a crash may prevent clearing. Encryption does not erase OS clipboard history, other apps' copies, old database pages, backups, or the persisted digest (a digest reveals content equality and permits offline guessing of low-entropy secrets; it is omitted from redacted JSON output). Legacy locked stickers gain a digest only after an authorized edit, unlock, or password rotation; until then identical external copies cannot be recognized without decrypting. Protect backups too.

## CLI

`rst` separates saved stickers, text workflows, and live previews. One command has one job; there are no legacy command aliases or implicit file-path commands. Start with `rst --help`, then `rst <command> --help`.

### Save and manage stickers

```bash
rst note                                      # blank editable note
rst note "Remember this"                       # Markdown note
rst note --file notes.md                       # exact UTF-8 file contents
rst note --file -                              # read stdin; empty note is valid
rst list                                      # all stickers, open and closed
rst list --type command --state open -s tests
rst show 42                                   # inspect stored content and settings
rst show 42 --content-only                     # exact content, no added newline
rst open 42
rst close 42
rst delete 42 --yes                            # permanent; explicit for scripts
```

Sticker IDs are positive integers. States are `all`, `open`, and `closed`. Creating a note opens it by default; `--closed` saves it without a window. Width and height must be positive; color names are validated. Deletion asks for confirmation only in an interactive terminal; JSON and redirected input require `--yes`.

### Command stickers

```bash
rst command --dir . --result text -- cargo test
rst command --title "Git status" --closed -- git status --short
rst command --shell "npm test 2>&1"            # explicit platform shell
rst command --cron "0 */5 * * * *" -- gh pr list
rst result 42                                 # latest stored output, not a new run
```

Put sticker options **before `--`**; everything after it is the program's exact argument list. No second layer of command-string quoting is needed. Direct mode never invokes a shell. `--shell SCRIPT` is exclusive with direct arguments; it uses PowerShell on Windows (`pwsh` when available) and `sh` elsewhere. Shell scripts are platform-specific code.

Creation **saves a command; it does not run it in the CLI or wait for output**. The desktop runs armed commands according to their lifecycle. `--idle` saves a stopped command; selection commands are triggered only by the hotkey. Schedules remain armed when their windows are closed and wait for their first tick after creation/app launch. `--run-now` requires a schedule and applies to a manual start from the sticker window, not creation or restoration. Cron uses six fields beginning with seconds, with an optional seventh year field.

Working directories are checked and pinned to absolute paths at creation, including the default current directory. Repeat `--env KEY=VALUE` to set environment variables; values retain whitespace, names and duplicates are checked. Missing executables are errors unless explicitly allowed with `--allow-missing-program`; malformed commands still fail. Inert or conflicting options are rejected rather than silently ignored. `--result` supports `text`, `markdown`, `html`, `svg`, and `source`; progressive `--stream` requires text or Markdown.

### Text to a program

```bash
rst text list                                 # saved notes + disposable captures
rst text list --source saved --search notes
rst text list --source clipboard --json
rst text exec -- my-tool '{{txt}}'             # choose interactively
rst text exec --id sticker:42 --non-interactive -- my-tool '{{txt}}'
rst text exec --source clipboard --id clipboard:42 --non-interactive -- my-tool
```

Both text commands default to `--source all --state all`. Source values are `saved`, `clipboard`, and `all`. Typed IDs (`sticker:42`, `clipboard:42`) prevent cross-table collisions. State filters apply to saved notes, not captures. Text listings are metadata/previews, not a plaintext export; locked entries expose only public titles/metadata.

Quote `{{txt}}` for your shell. It is replaced only in argument tokens, never the executable. Exact text stays one argument, including quotes, newlines, and shell characters; inserted text is not expanded recursively. Without a placeholder, text is appended as one argument. Child stdin/stdout/stderr are inherited, and its exit status is returned. `--json` is incompatible with `text exec`; a child's own `--json` after `--` remains its argument.

The chooser and password prompt use the controlling terminal, leaving child stdin untouched. For automation, use `--id` and `--non-interactive`; encrypted entries fail instead of prompting. Interactive decryption authorizes one use. Putting text into an interpreter's code argument can execute it, and sensitive argv can be visible to other processes.

Selection **stickers** use a different placeholder: `{{RUSTICKERS_SELECTION}}`, or the `RUSTICKERS_SELECTION` environment variable. Use `--accept-selection`; these stickers default closed. Shell code must read the environment variable rather than splice the placeholder into code.

### Recipes and agents

```bash
rst skill list
rst skill show translate
rst skill run translate --set language=Japanese --dry-run
rst skill run note --set "text=Remember this"
rst skill guide                               # operational Markdown guide for agents
rst skill guide --json                        # same guide in a JSON response
```

Recipes are bundled with the binary, expand to the same canonical commands, and are checked against the parser. `--set KEY=VALUE` is repeatable; unknown, duplicate, and missing required variables fail. `--dry-run` validates without creating storage or stickers. External tools are prerequisites, not installed automatically. `skill show` identifies incomplete recipes and platform requirements. Preview and execution share one expansion path; JSON argument arrays are authoritative, human command lines are quoted for the host shell.

### Automation contract

```bash
rst --json list
rst --data-dir ./scratch --json note "Experiment" --closed
rst --offline command --closed -- cargo test
```

- `--json` works before or after the subcommand and returns **one object on stdout** with `ok: true` or `ok: false`. Help/version are normal text. Grammar errors exit **2**, validation/runtime failures **1**, success **0**. `text exec` is the child-stdio exception, including cancellation **130** and launch errors **126/127**.
- `list` and `text list` return `items`, `count`, and `total`. IDs retain their command-specific integer/typed-string representation.
- Desktop notification is `sent`, `unavailable`, `offline`, or `not_requested`. `sent` means transport delivery, **not** that a window opened or execution finished. Stored command results distinguish `armed` from `has_output`.
- `--data-dir PATH` uses `PATH/stickers.db` and **always disables desktop IPC**, so experimental IDs cannot affect real windows. `--offline` disables IPC while using normal storage. Both propagate through recipe execution. Discovery, help, and dry-runs do not initialize storage.
- `view PATH_OR_URL` is a transient desktop-only request; it requires an existing source or URL and enabled IPC. It does not save a sticker.
- On Windows, Git Bash/MSYS can rewrite path-looking arguments before `rst` receives them. Set `MSYS_NO_PATHCONV=1` when literal child arguments such as `/c` must remain unchanged.

## Data

The app and CLI share `stickers.db`; logs live beside it in `logs/`. The CLI's `--data-dir` override is isolated and does not change the desktop's location.

- Windows: `%LOCALAPPDATA%\rustickers\data\`
- macOS: `~/Library/Application Support/rustickers/data/`
- Linux: `~/.local/share/rustickers/`

Startup imports usable raw text once from legacy clipboard storage, without recovering encrypted content. The old `clipboard.db` is left untouched and may contain sensitive data. Log level: `RUSTICKERS_LOG`, then `RUST_LOG`, then the build default.

## Contributing

See [ARCHITECTURE.md](ARCHITECTURE.md) for the system map and [FRONTEND.md](FRONTEND.md) for desktop conventions and checks.

CLI-only verification (no desktop dependencies):

```bash
cargo test --no-default-features --features cli
cargo clippy --no-default-features --features cli --all-targets -- -D warnings
```

Process tests use temporary stores and disabled IPC. The bundled agent guide and recipes are tested against the same parser as the binary.
