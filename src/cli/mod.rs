//! The `rusticker` command line.
//!
//! The CLI is the scripting surface for the desktop app: it creates stickers, finds them, opens
//! and closes them, and reads back what command stickers produced. Anything it changes goes into
//! the same database the app reads, so it works whether or not the app is running — when it is,
//! the change is also pushed over IPC and takes effect immediately.
//!
//! The module has two halves. [`commands`] holds one module per subcommand, each owning its own
//! arguments as well as its behaviour. Everything else is shared machinery: [`output`] for the
//! human/JSON split, [`runtime`] for the database and IPC, [`draft`] for the single sticker
//! creation path, [`shell`] for validating command strings, and [`skills`] for the catalogue of
//! worked examples.

pub mod commands;
mod draft;
mod output;
mod runtime;
mod shell;
mod skills;

use clap::{CommandFactory as _, Parser, Subcommand};

use crate::model::sticker::StickerColor;
use crate::storage::paths::AppPaths;

use output::Format;

pub(crate) fn parse_color(s: &str) -> Result<StickerColor, String> {
    let known = StickerColor::ALL.map(|c| c.as_str());
    if known.contains(&s.trim().to_ascii_lowercase().as_str()) {
        // `StickerColor::from_str` is infallible and quietly falls back to gray, so the membership
        // test above is what actually rejects a typo.
        s.parse::<StickerColor>()
            .map_err(|_| format!("invalid color '{s}'"))
    } else {
        Err(format!(
            "invalid color '{s}'; expected one of: {}",
            known.join(", ")
        ))
    }
}

const AFTER_HELP: &str = "\
Examples:
  rusticker list --state all                       every sticker
  rusticker markdown --file notes.md               pin a document
  rusticker cmd \"git status --short\" --dir .       show command output
  rusticker cmd \"npm test\" --shell                 use a shell for pipes and redirection
  rusticker result 12                              print what sticker 12 last produced
  rusticker skill list                             ready-made recipes worth copying

Command stickers do not run through a shell: the command is split with Windows argument rules and
the program is looked up on PATH. Pass --shell when you need pipes, redirection or `&&`.

Add --json to any command to get one machine-readable object on stdout instead of a report.";

#[derive(Parser)]
#[command(
    name = "rusticker",
    about = "Create and control Rustickers desktop stickers",
    long_about = "Create and control Rustickers desktop stickers.\n\nStickers live in a local \
                  database that the desktop app reads. Changes made here apply immediately when \
                  the app is running, and on next launch when it is not.",
    after_help = AFTER_HELP,
    version
)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Print one JSON object instead of a human-readable report
    ///
    /// The object always has an `ok` field, so success and failure parse the same way.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// List stickers
    List(commands::list::ListArgs),

    /// Show everything stored about one sticker
    Show(commands::show::ShowArgs),

    /// Print what a command sticker last produced
    Result(commands::result::ResultArgs),

    /// Open a sticker's window
    Open(commands::window::WindowArgs),

    /// Close a sticker's window
    Close(commands::window::WindowArgs),

    /// Delete a sticker permanently
    Delete(commands::delete::DeleteArgs),

    /// Preview a file, folder or URL
    View(commands::view::ViewArgs),

    /// Create a markdown note sticker
    Markdown(commands::markdown::MarkdownArgs),

    /// Create a command sticker
    Cmd(commands::cmd::CmdArgs),

    /// Worked examples: selection commands, scheduled runs, rendered output
    Skill(commands::skill::SkillArgs),
}

impl Cli {
    pub fn format(&self) -> Format {
        Format::new(self.json)
    }
}

/// Every subcommand name, so the file-path shorthand in the binary entry point cannot fall out of
/// date when a subcommand is added.
pub fn subcommand_names() -> Vec<String> {
    Cli::command()
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect()
}

pub fn run(cli: Cli, app_paths: &AppPaths) -> anyhow::Result<()> {
    let format = cli.format();

    match cli.command {
        Commands::List(args) => commands::list::run(app_paths, args, format),
        Commands::Show(args) => commands::show::run(app_paths, args, format),
        Commands::Result(args) => commands::result::run(app_paths, args, format),
        Commands::Open(args) => commands::window::open(app_paths, args, format),
        Commands::Close(args) => commands::window::close(app_paths, args, format),
        Commands::Delete(args) => commands::delete::run(app_paths, args, format),
        Commands::View(args) => commands::view::run(args, format),
        Commands::Markdown(args) => commands::markdown::run(app_paths, args, format),
        Commands::Cmd(args) => commands::cmd::run(app_paths, args, format),
        Commands::Skill(args) => commands::skill::run(app_paths, args, format),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_argument_definitions_are_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_subcommand_list_includes_every_command() {
        let names = subcommand_names();
        for expected in [
            "list", "show", "result", "open", "close", "delete", "view", "markdown", "cmd", "skill",
        ] {
            assert!(names.contains(&expected.to_owned()), "missing {expected}");
        }
    }

    #[test]
    fn json_is_accepted_before_and_after_the_subcommand() {
        assert!(
            Cli::parse_from(["rusticker", "--json", "list"])
                .format()
                .is_json()
        );
        assert!(
            Cli::parse_from(["rusticker", "list", "--json"])
                .format()
                .is_json()
        );
        assert!(!Cli::parse_from(["rusticker", "list"]).format().is_json());
    }

    #[test]
    fn colors_are_validated_against_the_real_palette() {
        assert!(parse_color("blue").is_ok());
        assert!(parse_color("  BLUE ").is_ok());
        let err = parse_color("chartreuse").unwrap_err();
        assert!(err.contains("expected one of"), "{err}");
    }

    #[test]
    fn every_skill_expands_to_arguments_the_parser_accepts() {
        // A skill is only trustworthy if the command it claims to run actually parses.
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
            let mut argv = vec!["rusticker".to_owned()];
            argv.extend(skill.expand(&vars));
            Cli::try_parse_from(&argv)
                .unwrap_or_else(|err| panic!("skill '{}' does not parse: {err}", skill.name));
        }
    }
}
