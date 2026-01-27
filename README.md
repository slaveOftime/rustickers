# Rustickers

A tiny desktop sticker app for quick notes, timers, and command outputs — built with [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) and backed by a local SQLite database.

- **Single instance**: launching again focuses the existing app
- **Global hotkey**: show the main window anytime. (ctrl+shift+s)
- **Persistent**: sticker windows restore on restart (position/size/state)

## What you can do

### Sticker types

| Type | What it’s for | Handy details |
| --- | --- | --- |
| **Text / Markdown** | Notes, checklists, snippets | Edit mode when empty; **Ctrl+S** saves; double‑click preview to edit |
| **Timer** | Reminders and quick countdowns | Sends a desktop notification when finished |
| **Command** | Pin the output of a command | Optional **cron** scheduling; supports env vars + working directory |

### Quality-of-life

- **Search & sort** in the main window (by created/updated time)
- **Color swatches** on sticker hover
- **Double‑click** a sticker card to open (or re-open) its window

## Hotkeys

- **Show main window**: `Ctrl + Shift + S`
  - On macOS: `Cmd + Shift + S` also works
- **Markdown sticker save**: `Ctrl + S` (while editing)

## Running

### From source (development)

Prerequisites:
- Rust (stable toolchain)

Build and run:

```bash
cargo run
```

### Build a release binary

```bash
cargo build --release
```

The executable will be in `target/release/` (Windows: `target\release\rustickers.exe`).

## Data storage

Rustickers stores everything in a local SQLite DB named `stickers.db` under your OS application data directory (via `directories::ProjectDirs`).

Typical locations:
- **Windows**: `%LOCALAPPDATA%\rustickers\data\stickers.db`
- **macOS**: `~/Library/Application Support/rustickers/data/stickers.db`
- **Linux**: `~/.local/share/rustickers/data/stickers.db`

## Releases

This repo’s GitHub Actions workflow builds zip artifacts for Windows, Linux, and macOS when you push a tag like `v0.1.0`.

## Project structure (high level)

- UI: GPUI windows in `src/windows/` and sticker components in `src/components/stickers/`
- Storage: SQLite implementation + migrations in `src/storage/` and `migrations/`
- Global hotkey + single-instance IPC: `src/hotkey.rs`, `src/ipc.rs`
