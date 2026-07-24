mod close;
mod cmd;
mod console;
mod list;
mod markdown;
mod open;
mod show;
pub mod view;

use clap::{Parser, Subcommand, ValueEnum};

use crate::model::sticker::StickerColor;
use crate::storage::paths::AppPaths;

fn parse_color(s: &str) -> Result<StickerColor, String> {
    s.parse::<StickerColor>().map_err(|_| {
        format!(
            "Invalid color '{s}'. Expected one of: {}",
            StickerColor::ALL
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

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

        /// Sticker width in pixels (default: auto-detected)
        #[arg(long)]
        width: Option<i32>,

        /// Sticker height in pixels (default: auto-detected)
        #[arg(long)]
        height: Option<i32>,

        /// Sticker color (yellow, green, blue, pink, gray)
        #[arg(long, value_parser = parse_color)]
        color: Option<StickerColor>,
    },

    /// Create and open a markdown sticker
    Markdown {
        /// Title for the sticker (defaults to first non-empty line of content)
        #[arg(long, short = 't')]
        title: Option<String>,

        /// Initial markdown content
        #[arg(long, short = 'c')]
        content: Option<String>,

        /// Sticker width in pixels (default: 400)
        #[arg(long)]
        width: Option<i32>,

        /// Sticker height in pixels (default: 300)
        #[arg(long)]
        height: Option<i32>,

        /// Sticker color (yellow, green, blue, pink, gray)
        #[arg(long, value_parser = parse_color)]
        color: Option<StickerColor>,
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

        /// Sticker width in pixels (default: 400)
        #[arg(long)]
        width: Option<i32>,

        /// Sticker height in pixels (default: 300)
        #[arg(long)]
        height: Option<i32>,

        /// Sticker color (yellow, green, blue, pink, gray)
        #[arg(long, value_parser = parse_color)]
        color: Option<StickerColor>,
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
        Commands::View {
            source,
            width,
            height,
            color,
        } => view::run(source, width, height, color),
        Commands::Markdown {
            content,
            title,
            width,
            height,
            color,
        } => markdown::run(app_paths, content, title, width, height, color),
        Commands::Cmd {
            command,
            cron,
            run_immediately,
            env,
            dir,
            width,
            height,
            color,
        } => cmd::run(
            app_paths,
            command,
            cron,
            run_immediately,
            env,
            dir,
            width,
            height,
            color,
        ),
    }
}
