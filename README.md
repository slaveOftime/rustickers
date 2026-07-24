# Rustickers

A lightweight desktop sticker app for notes, timers, command output, and file previews.

Built with [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), powered by local SQLite, and designed to stay fast and simple.

![Rustickers demo](./screenshots/demo1.png)

## Highlights

- **Markdown notes**: quick editing, preview mode, and **Ctrl+S** save.
- **Timers**: countdown timers with an audible alert on completion.
- **Command stickers**: pin command output, with optional cron scheduling, env vars, and working directory.
- **File / URL preview**: preview files, folders, or URLs and pin them as stickers.
- **Paint stickers**: freehand drawing space for quick sketches.

## Global hotkeys

- **Show main window**: `Ctrl + Alt + R` (macOS also supports `Cmd + Alt + R`)
- **Toggle quick file preview**: `Ctrl/Cmd + Alt`
- **Save in editor/markdown views**: `Ctrl + S`

## Quick start

Prerequisite: stable Rust toolchain.

Run the desktop app:

```bash
cargo run
```

Run the CLI:

```bash
cargo run --bin rusticker --no-default-features --features cli -- --help
```

Build release binaries:

```bash
cargo build --release --bin rustickers
cargo build --release --bin rusticker --no-default-features --features cli
```

Outputs are written to `target/release/`.

## CLI commands

`rusticker` supports:

- `list [--state open|close|all] [--search <text>]`
- `show <id>`
- `open <id>` / `close <id>`
- `view <path-or-url> [--width <px>] [--height <px>] [--color <color>]`
- `markdown [-t <title>] [-c <content>] [--width <px>] [--height <px>] [--color <color>]`
- `cmd <command> [--cron <expr>] [--run-now] [--env KEY=VALUE]... [--dir <path>] [--width <px>] [--height <px>] [--color <color>]`

Width and height default to 400×300 for markdown and command stickers, and auto-detected for file/URL stickers.
Color options: yellow, green, blue, pink, gray.

Tip: passing a single file path or URL is treated as `view <source>` automatically, and trailing options are forwarded.

## Data and logs

Rustickers stores data in `stickers.db` under your app data directory and writes daily-rotated logs to a sibling `logs/` directory.

Typical locations:

- **Windows**: `%LOCALAPPDATA%\rustickers\data\stickers.db` and `%LOCALAPPDATA%\rustickers\data\logs\`
- **macOS**: `~/Library/Application Support/rustickers/data/stickers.db` and `~/Library/Application Support/rustickers/data/logs/`
- **Linux**: `~/.local/share/rustickers/data/stickers.db` and `~/.local/share/rustickers/data/logs/`

Log level precedence:

1. `RUSTICKERS_LOG`
2. `RUST_LOG`
3. fallback: `trace` (debug) / `info` (release)

## Releases

Tagging (for example `v0.1.0`) triggers GitHub Actions release artifacts for Windows and macOS (UI + CLI bundles).
