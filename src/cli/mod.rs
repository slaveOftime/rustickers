mod close;
mod cmd;
mod console;
mod list;
mod open;
mod show;
pub mod view;

use clap::{Parser, Subcommand, ValueEnum};

use crate::storage::paths::AppPaths;

#[derive(Parser)]
#[command(name = "rustickers", about = "Rustickers sticker manager")]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Close an open sticker by ID
    Close {
        /// Sticker ID
        id: i64,
    },

    /// Open a closed sticker by ID
    Open {
        /// Sticker ID
        id: i64,
    },

    /// List stickers (id, title, type, position, size)
    List {
        /// Filter by state (default: open)
        #[arg(long, value_enum, default_value = "open")]
        state: ListState,

        /// Search in title and content
        #[arg(long)]
        search: Option<String>,
    },

    /// Show full detail for a sticker by ID
    Show {
        /// Sticker ID
        id: i64,
    },

    /// Create and open a file/URL sticker
    View {
        /// File path or URL to display
        source: String,
    },

    /// Create and open a command sticker
    Cmd {
        /// Shell command to run
        command: String,

        /// Cron expression for scheduling (e.g. "0 */1 * * * *" to run every minute)
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

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ListState {
    Open,
    Close,
    All,
}

pub fn run(cli: Cli, app_paths: &AppPaths) -> anyhow::Result<()> {
    match cli.command {
        Commands::Close { id } => close::run(id),
        Commands::Open { id } => open::run(id),
        Commands::List { state, search } => list::run(app_paths, state, search),
        Commands::Show { id } => show::run(app_paths, id),
        Commands::View { source } => view::run(source),
        Commands::Cmd {
            command,
            cron,
            run_immediately,
            env,
            dir,
        } => cmd::run(app_paths, command, cron, run_immediately, env, dir),
    }
}
