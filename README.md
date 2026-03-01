# Rustickers

A tiny desktop sticker app for quick notes, timers, and command outputs — built with [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) and backed by a local SQLite database.

![demo1](./screenshots/demo1.png)

## What you can do

### Sticker types

| Type | What it’s for | Handy details |
| --- | --- | --- |
| **Text / Markdown** | Notes, checklists, snippets | Edit mode when empty; **Ctrl+S** saves; double‑click preview to edit |
| **Timer** | Reminders and quick countdowns | Sends a desktop notification when finished |
| **Command** | Pin the output of a command | Optional **cron** scheduling; supports env vars + working directory |
| **Preview** | Preview files or URL and pin them | Can also edit small text or Markdown files |

## Hotkeys

- **Show main window**: `Ctrl + Alt + R`
  - On macOS: `Cmd + Alt + R` also works
- **Markdown sticker save**: `Ctrl + S` (while editing)

## Getting started

### From source (development)

Prerequisites:
- Rust (stable toolchain)

Build and run:

```bash
cargo run
```

Run the CLI:

```bash
cargo run --bin rusticker --no-default-features --features cli -- --help
```

### Build release binaries

```bash
cargo build --release --bin rustickers
cargo build --release --bin rusticker --no-default-features --features cli
```

Executables are generated in `target/release/` (Windows: `target\release\rustickers.exe` and `target\release\rusticker.exe`).

## Data storage

Rustickers stores data in a local SQLite database named `stickers.db` under your OS application data directory (via `directories::ProjectDirs`).

Typical locations:
- **Windows**: `%LOCALAPPDATA%\rustickers\data\stickers.db`
- **macOS**: `~/Library/Application Support/rustickers/data/stickers.db`
- **Linux**: `~/.local/share/rustickers/data/stickers.db`

## Logging

Rustickers writes logs to a daily-rotating file under the same app data directory as the database:

- **Windows**: `%LOCALAPPDATA%\rustickers\data\logs\`
- **macOS**: `~/Library/Application Support/rustickers/data/logs/`
- **Linux**: `~/.local/share/rustickers/data/logs/`

Log level can be configured with:

- `RUSTICKERS_LOG` (preferred), e.g. `RUSTICKERS_LOG=debug`
- `RUST_LOG` (fallback)

## Releases

GitHub Actions builds release artifacts when you push a tag like `v0.1.0`:

- **Windows**: two `.zip` assets — one for `rustickers.exe` (UI) and one for `rusticker.exe` (CLI)
- **Linux**: currently disabled in CI
- **macOS**: two `.zip` assets — one with `Rustickers.app` (UI), one with `rusticker` (CLI)
