mod close;
mod cmd;
mod console;
mod view;
mod list;
mod show;

use clap::{Parser, Subcommand};

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

pub fn run(cli: Cli, app_paths: &AppPaths) -> anyhow::Result<()> {
    console::setup_console();

    match cli.command {
        Commands::Close { id } => close::run(app_paths, id),
        Commands::List => list::run(app_paths),
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
