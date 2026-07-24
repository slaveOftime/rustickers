# Rustickers Architecture

A lightweight desktop sticker application for notes, timers, command output, and file previews. Built with [GPUI](https://github.com/zed-industries/zed), powered by SQLite.

## Table of Contents

- [Overview](#overview)
- [Project Structure](#project-structure)
- [Build Configuration](#build-configuration)
- [Module Architecture](#module-architecture)
  - [Entry Points](#entry-points)
  - [IPC Module](#ipc-module)
  - [Model Module](#model-module)
  - [Storage Module](#storage-module)
  - [Native Module](#native-module)
  - [CLI Module](#cli-module)
  - [Utils Module](#utils-module)
- [Data Flow](#data-flow)
- [Database Schema](#database-schema)
- [Sticker Types](#sticker-types)
- [Platform Support](#platform-support)
- [Key Design Patterns](#key-design-patterns)
- [Development Guide](#development-guide)

---

## Overview

Rustickers is a cross-platform desktop application that allows users to create persistent "stickers" on their desktop. Each sticker is a small window that can display notes, timers, command output, file previews, or freehand drawings.

### Core Features

- **Markdown Notes**: Quick editing with preview mode and Ctrl+S save
- **Timers**: Countdown timers with audible alerts
- **Command Stickers**: Pin command output with optional cron scheduling
- **File/URL Preview**: Preview files, folders, or URLs
- **Paint Stickers**: Freehand drawing space

### Technology Stack

- **UI Framework**: GPUI (Zed editor's UI framework)
- **WebView**: WRY (Webview Runtime) for HTML/PDF/URL rendering
- **Database**: SQLite with SQLx
- **IPC**: interprocess crate for single-instance enforcement
- **Hotkeys**: rdev for global hotkey listening
- **Logging**: tracing with daily rotation

---

## Project Structure

```
rustickers/
├── .cargo/
│   └── config.toml          # Platform-specific build config (static CRT on Windows)
├── .github/workflows/
│   └── release.yml          # CI/CD for release builds
├── .vscode/
│   └── settings.json        # Editor settings
├── assets/
│   ├── icons/               # 31 SVG icons for UI
│   ├── icon.ico, icon.png   # Application icons
│   └── design.pptx          # Design documents
├── migrations/              # SQLx database migrations
│   ├── 0001_init.sql        # Initial schema
│   ├── 0002_add_top_most.sql
│   └── 0003_add_display_id.sql
├── packaging/
│   └── linux/               # Linux desktop integration (.desktop, .service files)
├── screenshots/             # Application screenshots
├── src/
│   ├── lib.rs               # Library root, module declarations
│   ├── rustickers.rs        # GUI application entry point
│   ├── rusticker.rs         # CLI application entry point
│   ├── ipc.rs               # Inter-process communication
│   ├── cli/                 # CLI commands
│   ├── model/               # Data models
│   ├── native/              # GPUI native code
│   ├── storage/             # Persistence layer
│   └── utils/               # Utility functions
├── build.rs                 # Windows resource compilation (icon embedding)
├── Cargo.toml               # Project manifest
└── Cargo.lock               # Locked dependencies
```

---

## Build Configuration

### Features

```toml
[features]
default = ["ui"]
ui = [
    "dep:gpui", "dep:gpui_platform", "dep:gpui-component", "dep:gpui-wry",
    "dep:wry", "dep:sqlx", "dep:rdev", "dep:notify", "dep:rodio", ...
]
cli = ["dep:clap", "dep:futures", "dep:sqlx"]
```

### Binaries

| Binary | Path | Features | Description |
|--------|------|----------|-------------|
| `rustickers` | `src/rustickers.rs` | `ui` | Full GUI application |
| `rusticker` | `src/rusticker.rs` | `cli` | CLI-only tool |

### Release Optimization

```toml
[profile.release]
strip = true        # Strip symbols
opt-level = "z"     # Optimize for size
lto = true          # Link Time Optimization
codegen-units = 1   # Maximum optimization
panic = "abort"     # No stack unwinding
```

---

## Module Architecture

### Entry Points

#### GUI Application (`src/rustickers.rs`)

```
┌─────────────────────────────────────────────────────────────┐
│                     rustickers.rs                            │
│  ┌────────────────┐  ┌─────────────────┐  ┌──────────────┐ │
│  │ SingleInstance │  │ HotkeyListener  │  │ IPC Server   │ │
│  │ (lock acquire) │  │ (Ctrl+Alt+R)    │  │ (commands)   │ │
│  └────────────────┘  └─────────────────┘  └──────────────┘ │
│                           │                                  │
│                           ▼                                  │
│                   run_native()                               │
│                   ┌──────────────┐                          │
│                   │ MainWindow   │                          │
│                   │ StickerWindow│                          │
│                   └──────────────┘                          │
└─────────────────────────────────────────────────────────────┘
```

**Startup Flow:**
1. Initialize `AppPaths` (data directory resolution)
2. Setup logging with daily rotation
3. Acquire single-instance lock
4. Start IPC server thread
5. Start global hotkey listener
6. Run GPUI native loop

#### CLI Application (`src/rusticker.rs`)

```rust
fn main() {
    let app_paths = AppPaths::new();
    let cli = Cli::parse_from(normalize_argv_for_view_alias());
    cli::run(cli, &app_paths);
}
```

**Special Handling:** Single file path/URL argument is automatically aliased to `view <source>`. Trailing flags (like `--width`, `--height`, `--color`) are forwarded to the view command.

---

### IPC Module (`src/ipc.rs`)

**Purpose:** Single-instance enforcement and inter-process communication.

#### SingleInstance

Uses platform-specific named sockets:
- **Windows**: Named Pipes (`GenericNamespaced`)
- **Unix/macOS**: Unix domain sockets (`GenericFilePath`)

```rust
pub struct SingleInstance {
    listener: Option<interprocess::local_socket::Listener>,
}
```

**Corpse Socket Handling:** On Unix systems, stale socket files are automatically cleaned up on retry.

#### IpcEvent Commands

| Command | Format | Description |
|---------|--------|-------------|
| `Show` | `SHOW` | Activate main window |
| `ToggleFilePreview` | `TOGGLE_FILE_PREVIEW` | Quick file preview from selection |
| `OpenSticker` | `OPEN_STICKER <id>` | Open closed sticker by ID |
| `PreviewFile` | `PREVIEW_FILE <json>` | Create file/URL preview sticker. JSON payload: `{"source":"...","width":400,"height":300,"color":"yellow"}` |
| `CloseSticker` | `CLOSE_STICKER <id>` | Close sticker by ID |

---

### Model Module (`src/model/`)

#### Sticker Types (`sticker.rs`)

```rust
pub enum StickerType {
    Markdown,  // Text notes with markdown
    Timer,     // Countdown timer
    Command,   // Shell command output
    Paint,     // Freehand drawing
    File,      // File/URL preview
}

pub enum StickerState {
    Open,   // Visible on desktop
    Close,  // Hidden but persisted
}

pub enum StickerColor {
    Yellow, Green, Blue, Pink, Gray
}
```

#### Data Structures

**StickerBrief** - List view summary:
```rust
pub struct StickerBrief {
    pub id: i64,
    pub title: String,
    pub state: StickerState,
    pub color: StickerColor,
    pub sticker_type: StickerType,
    pub created_at: i64,
    pub updated_at: i64,
}
```

**StickerDetail** - Full sticker data:
```rust
pub struct StickerDetail {
    pub id: i64,
    pub title: String,
    pub state: StickerState,
    pub left: i32,      // Window position
    pub top: i32,
    pub width: i32,     // Window size
    pub height: i32,
    pub top_most: bool, // Always on top
    pub color: StickerColor,
    pub sticker_type: StickerType,
    pub content: String, // JSON-encoded type-specific content
    pub created_at: i64,
    pub updated_at: i64,
    pub display_id: Option<u32>, // Multi-monitor support
}
```

#### Content Types (`content.rs`)

**CommandContent:**
```rust
pub struct CommandContent {
    pub command: String,
    pub environments: String,     // KEY=VALUE pairs
    pub working_dir: String,
    pub scheduler: Option<Scheduler>, // Cron or interval
    pub run_immediately: bool,
    pub result: CommandResult,
    pub stream_result: bool,
}
```

**FileStickerContent:**
```rust
pub struct FileStickerContent {
    pub sources: Vec<String>,  // File paths or URLs
}
```

---

### Storage Module (`src/storage/`)

#### StickerStore Trait

```rust
#[async_trait::async_trait]
pub trait StickerStore: Send + Sync {
    async fn insert_sticker(&self, sticker: StickerDetail) -> anyhow::Result<i64>;
    async fn delete_sticker(&self, id: i64) -> anyhow::Result<()>;
    async fn get_sticker(&self, id: i64) -> anyhow::Result<StickerDetail>;
    async fn update_sticker_color(&self, id: i64, color: String) -> anyhow::Result<()>;
    async fn update_sticker_title(&self, id: i64, title: String) -> anyhow::Result<()>;
    async fn update_sticker_bounds(&self, id: i64, left: i32, top: i32, width: i32, height: i32, display_id: Option<u32>) -> anyhow::Result<()>;
    async fn update_sticker_content(&self, id: i64, content: String) -> anyhow::Result<()>;
    async fn update_sticker_state(&self, id: i64, state: StickerState) -> anyhow::Result<()>;
    async fn update_sticker_top_most(&self, id: i64, top_most: bool) -> anyhow::Result<()>;
    async fn query_stickers(&self, search: Option<String>, order_by: StickerOrderBy, limit: i64, offset: i64) -> anyhow::Result<Vec<StickerBrief>>;
    async fn count_stickers(&self, search: Option<String>) -> anyhow::Result<i64>;
    async fn get_open_sticker_ids(&self) -> anyhow::Result<Vec<i64>>;
    async fn list_stickers(&self, state: Option<StickerState>, search: Option<String>) -> anyhow::Result<Vec<StickerListItem>>;
}
```

#### SqliteStore Implementation

- Uses SQLx with compile-time query verification
- Connection pooling via `Arc<SqlitePool>`
- Migrations applied on first open

#### AppPaths

Cross-platform data directory resolution:

| Platform | Path |
|----------|------|
| Windows | `%LOCALAPPDATA%\rustickers\data\` |
| macOS | `~/Library/Application Support/rustickers/data/` |
| Linux | `~/.local/share/rustickers/data/` |

---

### Native Module (`src/native/`)

#### run_native()

GPUI application initialization:

```rust
pub fn run_native(
    app_paths: AppPaths,
    ipc_events_rx: mpsc::Receiver<IpcEvent>,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    sticker_events_rx: mpsc::Receiver<StickerWindowEvent>,
)
```

**Initialization Steps:**
1. Create GPUI Application with platform backend
2. Set dark theme
3. Spawn IPC event pump (20ms polling)
4. Open SQLite store
5. Restore open stickers from database
6. Open main window

#### Windows Module (`src/native/windows/`)

**MainWindow** - Sticker list/manager:
- Search and filter stickers
- Sort by created/updated date
- Create new stickers via dropdown menu
- Double-click to open, delete button to remove

**StickerWindow** - Individual sticker:
- Bounds persistence (debounced 200ms)
- Color picker footer
- Title editing with Enter to save
- Type-specific view component

#### Components Module (`src/native/components/`)

**Embedded Assets:**
```rust
#[derive(RustEmbed)]
#[folder = "./assets"]
#[include = "icons/**/*.svg"]
pub struct Assets;
```

**IconName Enum:** 31 predefined icons (play, pause, close, search, etc.)

#### Sticker Implementations (`src/native/components/stickers/`)

| Sticker | File | Features |
|---------|------|----------|
| Markdown | `markdown.rs` | Edit/preview modes, syntax highlighting, Ctrl+S save |
| Timer | `timer.rs` | Countdown, audible alert, pause/reset |
| Command | `command.rs` | Shell execution, cron scheduling, env vars, streaming output |
| Paint | `paint.rs` | Freehand drawing, eraser, color selection |
| File | `file/mod.rs` | Multi-file preview, file watching, type-specific renderers |

**File Sticker Sub-components:**
- `preview.rs` - File type detection and routing
- `editor.rs` - Text/code editor with syntax highlighting
- `audio.rs` - Audio player with metadata display
- `watcher.rs` - File system change notifications
- `summary.rs` - File/folder summary display

#### WebView Wrapper (`src/native/components/webview.rs`)

WRY-based WebView for rendering:
- HTML content
- PDF files
- URLs (via local:// protocol)

#### Hotkey Module (`src/native/hotkey.rs`)

Global hotkey listener using rdev:

| Hotkey | Action |
|--------|--------|
| `Ctrl + Alt + R` | Show main window |
| `Ctrl/Cmd + Alt` | Toggle file preview from selection |

**Platform-specific modifiers:**
- Windows/Linux: Ctrl required
- macOS: Cmd or Ctrl accepted

#### File Manager Integration (`src/native/file_manager.rs`)

Extracts selected files from:
- **Windows**: Explorer windows (including tabs)
- **macOS**: Finder selection via AppleScript

#### HTTP Client (`src/native/http.rs`)

Reqwest wrapped for GPUI's `HttpClient` trait.

---

### CLI Module (`src/cli/`)

#### Commands

| Command | File | Description |
|---------|------|-------------|
| `list` | `list.rs` | List stickers with filters |
| `show` | `show.rs` | Show full sticker details |
| `open` | `open.rs` | Open closed sticker |
| `close` | `close.rs` | Close open sticker |
| `view` | `view.rs` | Create file/URL preview sticker |
| `markdown` | `markdown.rs` | Create markdown note sticker |
| `cmd` | `cmd.rs` | Create command sticker |

#### CLI Flow

```rust
pub fn run(cli: Cli, app_paths: &AppPaths) -> anyhow::Result<()> {
    match cli.command {
        Commands::Close { id } => close::run(id),
        Commands::Open { id } => open::run(id),
        Commands::List { state, search } => list::run(app_paths, state, search),
        Commands::Show { id } => show::run(app_paths, id),
        Commands::View { source, width, height, color } => view::run(source, width, height, color),
        Commands::Markdown { content, title, width, height, color } =>
            markdown::run(app_paths, content, title, width, height, color),
        Commands::Cmd { .. } => cmd::run(app_paths, command, cron, run_immediately, env, dir, width, height, color),
    }
}
```

---

### Utils Module (`src/utils/`)

#### Logging (`logging.rs`)

```rust
pub struct LoggingGuards {
    _file_guard: tracing_appender::non_blocking::WorkerGuard,
    _error_guard: Option<tracing_error::ErrorLayer>,
}
```

**Configuration:**
- Daily rotation
- Environment variables: `RUSTICKERS_LOG` > `RUST_LOG` > default
- Debug builds: `trace`
- Release builds: `info`

#### File Utilities (`file.rs`)

File type detection by extension:
- Images: png, jpg, jpeg, gif, webp, svg, ico, bmp
- Video: mp4, webm, mkv, avi, mov, flv, wmv
- Audio: mp3, wav, ogg, flac, aac, m4a
- Code: rs, py, js, ts, go, java, cpp, c, h, cs, php, rb, sh
- Documents: md, txt, json, yaml, toml, xml, html, css

#### URL Utilities (`url.rs`)

- URL detection regex
- `local://` protocol helper for WebView

#### Time Utilities (`time.rs`)

- Unix timestamp in milliseconds
- Human-readable formatting

---

## Data Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                        rustickers.rs                             │
│  ┌─────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ SingleInstance│  │ HotkeyListener│  │ IPC Server Thread    │ │
│  └──────┬──────┘  └──────┬───────┘  └───────────┬────────────┘ │
│         │                │                       │              │
│         └────────────────┴───────────────────────┘              │
│                          │                                      │
│                   IpcEvent Channel                              │
│                          │                                      │
│         ┌────────────────▼────────────────┐                     │
│         │        run_native()             │                     │
│         │  ┌──────────┐  ┌────────────┐  │                     │
│         │  │ MainWindow│  │StickerWindow│  │                     │
│         │  └────┬─────┘  └─────┬──────┘  │                     │
│         │       │              │         │                     │
│         │       └──────┬───────┘         │                     │
│         │              ▼                 │                     │
│         │    ┌─────────────────┐         │                     │
│         │    │  StickerStore   │         │                     │
│         │    │  (SqliteStore)  │         │                     │
│         │    └────────┬────────┘         │                     │
│         └─────────────┼──────────────────┘                     │
└───────────────────────┼─────────────────────────────────────────┘
                        │
                        ▼
              ┌─────────────────┐
              │ stickers.db     │
              │ (SQLite)        │
              └─────────────────┘
```

### Event Channels

1. **IPC Events** (`mpsc::Sender<IpcEvent>`): External commands from CLI or second instance
2. **Sticker Events** (`mpsc::Sender<StickerWindowEvent>`): Internal state sync between windows

### StickerWindowEvent

```rust
pub enum StickerWindowEvent {
    Created { id: i64 },
    TitleChanged { id: i64, title: String },
    ColorChanged { id: i64, color: StickerColor },
    Closed { id: i64 },
}
```

---

## Database Schema

### Migrations

#### 0001_init.sql

```sql
CREATE TABLE IF NOT EXISTS stickers (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    title       TEXT NOT NULL,
    state       TEXT NOT NULL,
    left        INTEGER NOT NULL,
    top         INTEGER NOT NULL,
    width       INTEGER NOT NULL,
    height      INTEGER NOT NULL,
    color       TEXT NOT NULL,
    type        TEXT NOT NULL,
    content     TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_stickers_created_at ON stickers(created_at);
CREATE INDEX IF NOT EXISTS idx_stickers_updated_at ON stickers(updated_at);
CREATE INDEX IF NOT EXISTS idx_stickers_title      ON stickers(title);
```

#### 0002_add_top_most.sql

```sql
ALTER TABLE stickers ADD COLUMN top_most INTEGER NOT NULL DEFAULT 0;
```

#### 0003_add_display_id.sql

```sql
ALTER TABLE stickers ADD COLUMN display_id INTEGER;
```

---

## Sticker Types

### Markdown Sticker

- **Content**: Raw markdown text
- **Features**: Edit/preview toggle, syntax highlighting, Ctrl+S save
- **Default Size**: 400x300

### Timer Sticker

- **Content**: JSON with duration, remaining time, running state
- **Features**: Countdown, pause/resume, audible alert
- **Default Size**: 200x100

### Command Sticker

- **Content**: `CommandContent` JSON
- **Features**: Shell execution, cron scheduling, environment variables, working directory, streaming output
- **Default Size**: 400x300

### Paint Sticker

- **Content**: Serialized drawing data
- **Features**: Freehand drawing, eraser, color selection
- **Default Size**: 300x300

### File Sticker

- **Content**: `FileStickerContent` JSON with sources array
- **Features**: Multi-file preview, file watching, type-specific renderers (image, video, audio, text, PDF)
- **Default Size**: Varies by content

---

## Platform Support

| Platform | Status | Notes |
|----------|--------|-------|
| **Windows** | Full | Static CRT linking, icon embedding, Explorer integration, Named Pipes IPC |
| **macOS** | Full | Finder integration via AppleScript, Cmd+Alt hotkeys, .app bundle |
| **Linux** | Partial | GTK/Webkit dependencies, .desktop file, .service file provided |

### Platform-Specific Code

**Windows:**
```rust
#[cfg(target_os = "windows")]
use windows::Win32::UI::Shell::{IShellWindows, IWebBrowserApp, ...};
```

**macOS:**
```rust
#[cfg(target_os = "macos")]
use cocoa::foundation::{NSAutoreleasePool, NSString};
#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};
```

---

## Key Design Patterns

### 1. Trait-based Storage

`StickerStore` trait allows swapping storage backends without changing business logic.

### 2. Entity-Component UI

GPUI-based with `Entity<T>` for reactive state management. Views re-render automatically on state changes.

### 3. Channel-based IPC

`mpsc` channels for cross-thread communication between:
- IPC server thread → Main thread
- Hotkey listener → Main thread
- Sticker windows → Main thread

### 4. Debounced Persistence

Window bounds saved after 200ms of inactivity to avoid excessive writes.

### 5. Single-instance Enforcement

Named socket prevents multiple app instances. Corpse socket cleanup on Unix.

### 6. Feature Flags

Separate UI and CLI builds from same codebase via Cargo features.

### 7. Content Serialization

Sticker content stored as JSON strings, allowing type-specific data without schema changes.

---

## Development Guide

### Adding a New Sticker Type

1. **Define content structure** in `src/model/content.rs`:
   ```rust
   pub struct NewStickerContent {
       // fields
   }
   ```

2. **Add to StickerType enum** in `src/model/sticker.rs`:
   ```rust
   pub enum StickerType {
       // ...
       NewType,
   }
   ```

3. **Create sticker component** in `src/native/components/stickers/new_type.rs`:
   ```rust
   pub struct NewTypeSticker { /* ... */ }
   impl Sticker for NewTypeSticker { /* ... */ }
   impl StickerView for NewTypeSticker { /* ... */ }
   ```

4. **Register in StickerWindow::create_sticker_view()**:
   ```rust
   StickerType::NewType => Box::new(StickerViewEntity::new(cx.new(|cx| {
       NewTypeSticker::new(id, color, store, content, window, cx, sticker_events_tx)
   }))),
   ```

5. **Add migration** if new database fields needed:
   ```bash
   sqlx migrate add add_new_type_fields
   ```

6. **Add icon** to `assets/icons/` and `IconName` enum

### Adding a New CLI Command

1. **Add to Commands enum** in `src/cli/mod.rs`:
   ```rust
   NewCommand {
       #[arg(long)]
       option: String,
   }
   ```

2. **Create module** `src/cli/new_command.rs`:
   ```rust
   pub fn run(app_paths: &AppPaths, option: String) -> anyhow::Result<()> {
       // implementation
   }
   ```

3. **Dispatch in run()**:
   ```rust
   Commands::NewCommand { option } => new_command::run(app_paths, option),
   ```

### Debugging

**Enable verbose logging:**
```bash
RUSTICKERS_LOG=trace cargo run
```

**View logs:**
- Windows: `%LOCALAPPDATA%\rustickers\data\logs\`
- macOS: `~/Library/Application Support/rustickers/data/logs/`
- Linux: `~/.local/share/rustickers/data/logs/`

**Database inspection:**
```bash
sqlite3 "$(pwd)/target/debug/data/stickers.db"
```

### Testing IPC

**Send command to running instance:**
```bash
echo "SHOW" | nc -U /tmp/rustickers-<user>.sock  # Unix
echo "SHOW" > \\.\pipe\rustickers-<user>         # Windows
```

---

## Glossary

| Term | Definition |
|------|------------|
| **Sticker** | A persistent desktop window displaying content |
| **StickerWindow** | GPUI window entity for a single sticker |
| **MainWindow** | Main application window showing sticker list |
| **IpcEvent** | Inter-process communication command |
| **StickerStore** | Trait for sticker persistence |
| **AppPaths** | Cross-platform data directory resolver |

---

## References

- [GPUI Documentation](https://github.com/zed-industries/zed/tree/main/crates/gpui)
- [SQLx Documentation](https://docs.rs/sqlx)
- [interprocess Documentation](https://docs.rs/interprocess)
- [rdev Documentation](https://docs.rs/rdev)
- [WRY Documentation](https://docs.rs/wry)
