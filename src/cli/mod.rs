//! Human and agent command interface. Parsing, execution and presentation share one contract.

pub mod commands;
mod draft;
mod output;
pub(crate) mod runtime;
mod shell;
mod skills;

use crate::{model::sticker::StickerColor, storage::paths::AppPaths};
use clap::{CommandFactory as _, Parser, Subcommand};
use output::Format;
use runtime::Runtime;
use std::path::PathBuf;

pub(crate) fn parse_color(s: &str) -> Result<StickerColor, String> {
    let known = StickerColor::ALL.map(|c| c.as_str());
    let normalized = s.trim().to_ascii_lowercase();
    if known.contains(&normalized.as_str()) {
        normalized
            .parse()
            .map_err(|_| format!("invalid color '{s}'"))
    } else {
        Err(format!(
            "invalid color '{s}'; expected: {}",
            known.join(", ")
        ))
    }
}

pub(crate) fn parse_id(s: &str) -> Result<i64, String> {
    s.parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| "sticker ID must be a positive integer".to_owned())
}

pub(crate) fn parse_dimension(s: &str) -> Result<i32, String> {
    s.parse::<i32>()
        .ok()
        .filter(|size| *size > 0)
        .ok_or_else(|| "size must be a positive number of pixels".to_owned())
}

const AFTER_HELP: &str = "\
Examples:
  rst list                                  all stickers, open and closed
  rst note \"Remember this\"                  create a Markdown note
  rst note --file notes.md                   read a document (use - for stdin)
  rst command --dir . -- cargo test          save a direct command
  rst command --shell \"git status | head\"   explicitly use the platform shell
  rst text list --search notes --json        text/clipboard metadata
  rst text exec --id sticker:42 -- tool '{{txt}}'
  rst skill guide                           agent workflow and safety contract

Creation saves a sticker; only the desktop app runs command stickers.
Use --json for one JSON response. text exec instead inherits child stdio and exit status.
--data-dir PATH selects an isolated store and disables desktop IPC.";

#[derive(Parser)]
#[command(name = "rst", about = "Notes, command stickers, and text workflows for Rustickers",
    after_help = AFTER_HELP, version)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Emit one JSON response (including errors); incompatible with text exec
    #[arg(long, global = true)]
    json: bool,

    /// Use PATH/stickers.db and disable desktop IPC (safe for scripts and experiments)
    #[arg(long, global = true, value_name = "PATH")]
    data_dir: Option<PathBuf>,

    /// Save changes without notifying the running desktop app
    #[arg(long, global = true)]
    offline: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// List stickers (all states by default)
    List(commands::list::ListArgs),
    /// Inspect a sticker, or print its content with --content-only
    Show(commands::show::ShowArgs),
    /// Print the latest stored command output (does not execute or wait)
    Result(commands::result::ResultArgs),
    /// Mark a sticker open and request its window
    Open(commands::window::WindowArgs),
    /// Mark a sticker closed; schedules remain armed
    Close(commands::window::WindowArgs),
    /// Permanently delete a sticker (--yes required outside a terminal)
    Delete(commands::delete::DeleteArgs),
    /// Request a transient file, folder, or URL preview (desktop required)
    View(commands::view::ViewArgs),
    /// Create a Markdown note from text, file, or stdin
    Note(commands::note::NoteArgs),
    /// Save a command sticker; use -- PROGRAM ARGS or --shell SCRIPT
    Command(commands::command::CommandArgs),
    /// List text entries or pass one to a child program
    #[command(subcommand)]
    Text(commands::text::TextArgs),
    /// Discover, preview, and create built-in recipes; guide for agents
    Skill(commands::skill::SkillArgs),
}

impl Cli {
    /// Clap propagates global values after local conflict checks. Check the cross-command
    /// contract here too, so `--json text exec` and `text exec --json` behave identically.
    pub fn try_parse_from<I, T>(args: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let cli = <Self as Parser>::try_parse_from(args)?;
        if cli.json
            && matches!(
                cli.command,
                Commands::Text(commands::text::TextArgs::Exec(_))
            )
        {
            return Err(clap::Error::raw(
                clap::error::ErrorKind::ArgumentConflict,
                "--json cannot be combined with text exec; the child owns stdout",
            ));
        }
        Ok(cli)
    }

    pub fn format(&self) -> Format {
        Format::new(self.json)
    }

    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        if let Commands::Command(args) = &self.command {
            args.validate()?;
        }
        Ok(())
    }

    pub(crate) fn execute(self, runtime: &Runtime) -> anyhow::Result<()> {
        let format = self.format();
        match self.command {
            Commands::List(args) => commands::list::run(runtime, args, format),
            Commands::Show(args) => commands::show::run(runtime, args, format),
            Commands::Result(args) => commands::result::run(runtime, args, format),
            Commands::Open(args) => commands::window::open(runtime, args, format),
            Commands::Close(args) => commands::window::close(runtime, args, format),
            Commands::Delete(args) => commands::delete::run(runtime, args, format),
            Commands::View(args) => commands::view::run(runtime, args, format),
            Commands::Note(args) => commands::note::run(runtime, args, format),
            Commands::Command(args) => commands::command::run(runtime, args, format),
            Commands::Text(args) => commands::text::run(runtime, args, format),
            Commands::Skill(args) => commands::skill::run(runtime, args, format),
        }
    }

    /// Execute with platform-default storage, unless --data-dir selects an isolated store.
    pub fn run(self) -> anyhow::Result<()> {
        // Pure discovery does not depend on a writable user-data directory.
        let paths = if let Some(dir) = &self.data_dir {
            AppPaths {
                db_path: dir.join("stickers.db"),
            }
        } else {
            AppPaths::default_location()?
        };
        let offline = self.offline || self.data_dir.is_some();
        self.execute(&Runtime::new(paths, offline))
    }
}

pub fn subcommand_names() -> Vec<String> {
    Cli::command()
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect()
}

/// The library test seam is offline by default; explicit CLI --data-dir is always offline.
pub fn run(cli: Cli, paths: &AppPaths) -> anyhow::Result<()> {
    let paths = cli
        .data_dir
        .as_ref()
        .map(|dir| AppPaths {
            db_path: dir.join("stickers.db"),
        })
        .unwrap_or_else(|| paths.clone());
    cli.execute(&Runtime::new(paths, true))
}

fn collect_value_flags(command: &clap::Command, flags: &mut Vec<String>) {
    for arg in command
        .get_arguments()
        .filter(|arg| arg.get_action().takes_values())
    {
        if let Some(long) = arg.get_long() {
            flags.push(format!("--{long}"));
        }
        if let Some(short) = arg.get_short() {
            flags.push(format!("-{short}"));
        }
    }
    for subcommand in command.get_subcommands() {
        collect_value_flags(subcommand, flags);
    }
}

/// Parse without Clap exiting so JSON clients also receive structured usage failures.
pub fn main_entry(args: impl IntoIterator<Item = std::ffi::OsString>) -> i32 {
    let args: Vec<_> = args.into_iter().collect();
    // Respect option values as well as the child delimiter: `--title --json` is a title,
    // not a request to change the error stream. Build this list from Clap, not a flag cache.
    let mut value_flags = Vec::new();
    collect_value_flags(&Cli::command(), &mut value_flags);
    let mut json = false;
    let mut skip_value = false;
    for arg in args.iter().skip(1) {
        if skip_value {
            skip_value = false;
            continue;
        }
        if arg == "--" {
            break;
        }
        if arg == "--json" {
            json = true;
        }
        if let Some(arg) = arg.to_str() {
            skip_value = value_flags.iter().any(|flag| flag == arg);
        }
    }
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                let _ = error.print();
                return 0;
            }
            if json {
                Format::Json.emit_error(&anyhow::anyhow!(error.to_string()));
            } else {
                let _ = error.print();
            }
            return 2;
        }
    };
    let format = cli.format();
    match cli.run() {
        Ok(()) => 0,
        Err(error) => {
            if let Some(code) = error.downcast_ref::<commands::text_execution::ExitCode>() {
                return code.0;
            }
            format.emit_error(&error);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn definitions_are_consistent() {
        Cli::command().debug_assert();
    }
    #[test]
    fn colors_normalize_before_parsing() {
        assert_eq!(parse_color(" BLUE ").unwrap(), StickerColor::Blue);
        assert!(parse_color("typo").is_err());
    }
    #[test]
    fn old_grammar_is_removed() {
        for name in ["ls", "cmd", "markdown", "cp", "vault"] {
            assert!(Cli::try_parse_from(["rst", name]).is_err());
        }
    }
    #[test]
    fn every_skill_matches_the_parser() {
        for skill in skills::SKILLS {
            let vars = skill
                .vars
                .iter()
                .map(|var| {
                    (
                        var.name.to_owned(),
                        var.default.unwrap_or("placeholder").to_owned(),
                    )
                })
                .collect();
            let mut argv = vec!["rst".to_owned()];
            argv.extend(skill.expand(&vars));
            Cli::try_parse_from(argv).unwrap_or_else(|error| panic!("{}: {error}", skill.name));
        }
    }
}
