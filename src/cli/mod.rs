use std::io::Write as _;

use anyhow::Context as _;
use clap::{Parser, Subcommand};

use crate::ipc;
use crate::model::content::{CommandContent, CommandResult, Scheduler};
use crate::model::sticker::{StickerColor, StickerDetail, StickerState, StickerType};
use crate::storage::paths::AppPaths;

#[derive(Parser)]
#[command(name = "rustickers", about = "Rustickers sticker manager")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Close an open sticker by ID
    Close {
        /// Sticker ID
        id: i64,
    },

    /// List all open stickers (id, title, type, position, size)
    List,

    /// Show full detail for a sticker by ID
    Show {
        /// Sticker ID
        id: i64,
    },

    /// Create and open a file/URL sticker
    File {
        /// File path or URL to display
        source: String,
    },

    /// Create and open a command sticker
    Cmd {
        /// Shell command to run
        command: String,

        /// Cron expression for scheduling (e.g. "*/5 * * * *")
        #[arg(long)]
        cron: Option<String>,

        /// Run command immediately on creation
        #[arg(long = "run-now", action = clap::ArgAction::SetTrue)]
        run_immediately: bool,

        /// Environment variables as KEY=VALUE (repeatable)
        #[arg(long, value_name = "KEY=VALUE")]
        env: Vec<String>,

        /// Working directory for the command
        #[arg(long)]
        dir: Option<String>,
    },
}

pub fn run(cli: Cli, app_paths: &AppPaths) -> anyhow::Result<()> {
    setup_console();

    match cli.command {
        Commands::Close { id } => cmd_close(app_paths, id),
        Commands::List => cmd_list(app_paths),
        Commands::Show { id } => cmd_show(app_paths, id),
        Commands::File { source } => cmd_file(app_paths, source),
        Commands::Cmd {
            command,
            cron,
            run_immediately,
            env,
            dir,
        } => cmd_command(app_paths, command, cron, run_immediately, env, dir),
    }
}

// ── Console setup ────────────────────────────────────────────────────────────

/// On Windows, attach to the parent process console so that `println!` output
/// is visible when the binary (built as `windows_subsystem = "windows"`) is
/// invoked from a terminal.
fn setup_console() {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// Returns a writer that reaches the console.
///
/// On Windows (GUI subsystem) stdout may not be connected after `AttachConsole`
/// so we open `CONOUT$` directly, which always maps to the active console buffer.
fn console_writer() -> Box<dyn std::io::Write> {
    #[cfg(target_os = "windows")]
    {
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            return Box::new(f);
        }
    }
    Box::new(std::io::stdout())
}

// ── Commands ─────────────────────────────────────────────────────────────────

fn cmd_list(app_paths: &AppPaths) -> anyhow::Result<()> {
    let store = block_on(crate::storage::open_sqlite(&app_paths.db_path))?;
    let ids = block_on(store.get_open_sticker_ids())?;

    let mut out = console_writer();

    if ids.is_empty() {
        writeln!(out, "No open stickers.")?;
        return Ok(());
    }

    writeln!(
        out,
        "{:<6}  {:<30}  {:<10}  {:<12}  {}",
        "ID", "TITLE", "TYPE", "POSITION", "SIZE"
    )?;
    writeln!(out, "{}", "-".repeat(70))?;

    for id in ids {
        match block_on(store.get_sticker(id)) {
            Ok(d) => {
                let title = truncate(&d.title, 30);
                let type_str = format!("{:?}", d.sticker_type);
                let pos = format!("{},{}", d.left, d.top);
                let size = format!("{}x{}", d.width, d.height);
                writeln!(
                    out,
                    "{:<6}  {:<30}  {:<10}  {:<12}  {}",
                    id, title, type_str, pos, size
                )?;
            }
            Err(err) => {
                writeln!(out, "{:<6}  [error: {}]", id, err)?;
            }
        }
    }

    Ok(())
}

fn cmd_close(app_paths: &AppPaths, id: i64) -> anyhow::Result<()> {
    let mut out = console_writer();

    // Try the running instance first so the window is also removed.
    match ipc::send_ipc_command("rustickers", &format!("CLOSE_STICKER {id}")) {
        Ok(true) => {
            return Ok(());
        }
        Ok(false) => {} // app not running — fall through to direct DB update
        Err(err) => {
            tracing::warn!(error = %err, "IPC send failed; falling back to direct DB update");
        }
    }

    // App is not running: update DB state directly.
    let store = block_on(crate::storage::open_sqlite(&app_paths.db_path))?;
    block_on(store.update_sticker_state(id, crate::model::sticker::StickerState::Close))
        .with_context(|| format!("failed to close sticker {id}"))?;
    writeln!(
        out,
        "Closed sticker {id} (app was not running; state updated in DB)."
    )?;
    Ok(())
}

fn cmd_show(app_paths: &AppPaths, id: i64) -> anyhow::Result<()> {
    let store = block_on(crate::storage::open_sqlite(&app_paths.db_path))?;
    let d = block_on(store.get_sticker(id)).with_context(|| format!("sticker {} not found", id))?;

    let mut out = console_writer();
    writeln!(out, "id:         {}", d.id)?;
    writeln!(out, "title:      {}", d.title)?;
    writeln!(out, "type:       {:?}", d.sticker_type)?;
    writeln!(out, "state:      {:?}", d.state)?;
    writeln!(out, "color:      {}", d.color.as_str())?;
    writeln!(out, "position:   left={}, top={}", d.left, d.top)?;
    writeln!(out, "size:       {}x{}", d.width, d.height)?;
    writeln!(out, "top_most:   {}", d.top_most)?;
    writeln!(out, "created_at: {}", format_ts(d.created_at))?;
    writeln!(out, "updated_at: {}", format_ts(d.updated_at))?;
    writeln!(out, "content:")?;
    writeln!(out, "{}", d.content)?;

    Ok(())
}

fn cmd_file(_app_paths: &AppPaths, source: String) -> anyhow::Result<()> {
    setup_console();
    let mut out = console_writer();
    match ipc::send_ipc_command("rustickers", &format!("PREVIEW_FILE {source}")) {
        Ok(true) => {}
        Ok(false) => {
            writeln!(
                out,
                "Rustickers is not running. Please launch it first, then retry."
            )?;
            std::process::exit(1);
        }
        Err(err) => {
            writeln!(out, "error: {err}")?;
            std::process::exit(1);
        }
    }
    Ok(())
}

fn cmd_command(
    app_paths: &AppPaths,
    command: String,
    cron: Option<String>,
    run_immediately: bool,
    env: Vec<String>,
    dir: Option<String>,
) -> anyhow::Result<()> {
    let store = block_on(crate::storage::open_sqlite(&app_paths.db_path))?;

    let content = CommandContent {
        command: command.clone(),
        environments: env.join("\n"),
        working_dir: dir.unwrap_or_default(),
        scheduler: cron.map(Scheduler::Cron),
        run_immediately,
        result: CommandResult::Text(None),
        stream_result: false,
        padding: None,
        started_at: None,
    };

    let content_json = serde_json::to_string(&content)?;
    let title = truncate(&command, 30).to_owned();

    let id = block_on(store.insert_sticker(StickerDetail {
        id: 0,
        title,
        state: StickerState::Open,
        left: 100,
        top: 100,
        width: 400,
        height: 300,
        top_most: false,
        color: StickerColor::Gray,
        sticker_type: StickerType::Command,
        content: content_json,
        created_at: 0,
        updated_at: 0,
    }))?;

    let mut out = console_writer();
    writeln!(out, "Created command sticker (id={id})")?;
    signal_open(id, &mut *out);
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn signal_open(id: i64, out: &mut dyn std::io::Write) {
    match ipc::send_ipc_command("rustickers", &format!("OPEN_STICKER {id}")) {
        Ok(true) => {
            let _ = writeln!(out, "Signaled running Rustickers to open the sticker.");
        }
        Ok(false) => {
            let _ = writeln!(
                out,
                "Rustickers is not running — sticker will open on next launch."
            );
        }
        Err(err) => {
            let _ = writeln!(out, "Note: could not signal running instance: {err}");
        }
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}

fn format_ts(ts: i64) -> String {
    use chrono::{DateTime, Utc};
    DateTime::from_timestamp_millis(ts)
        .map(|dt: DateTime<Utc>| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| ts.to_string())
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}
