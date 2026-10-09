//! `rst text` — the TXTs collection: saved Markdown text and disposable clipboard captures.
//!
//! Two actions, deliberately separate. `list` reads metadata and never runs anything; `exec`
//! resolves exactly one entry and hands it to a child program as argv data. Nothing about
//! execution is reachable through `list`, so a script can enumerate freely and only opt into
//! child processes explicitly.

use clap::{Subcommand, ValueEnum};
use serde_json::json;

use crate::text::{TextEntry, TextEntryId, TextSources};

use crate::cli::output::{Format, one_line, table};
use crate::cli::runtime::{Runtime, block_on};

use super::list::{StateFilter, parse_limit, state_name};

/// Which halves of the collection to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SourceFilter {
    /// Saved Markdown stickers
    Saved,
    /// Disposable clipboard captures
    Clipboard,
    All,
}

impl SourceFilter {
    fn sources(self) -> TextSources {
        TextSources::new(
            matches!(self, Self::Saved | Self::All),
            matches!(self, Self::Clipboard | Self::All),
        )
    }
}

#[derive(Subcommand, Debug)]
pub enum TextArgs {
    /// List saved text and clipboard captures (redacted metadata)
    List(TextListArgs),
    /// Run a program with exactly one text entry as argument data
    Exec(TextExecArgs),
}

#[derive(clap::Args, Debug)]
pub struct TextListArgs {
    /// Which entries to include
    #[arg(long, value_enum, default_value = "all")]
    pub source: SourceFilter,

    /// Saved stickers in this window state (captures have no state)
    #[arg(long, value_enum, default_value = "all")]
    pub state: StateFilter,

    /// Match against the title and the content
    #[arg(long, short = 's', value_name = "TEXT")]
    pub search: Option<String>,

    /// Show at most this many entries
    #[arg(long, short = 'n', value_name = "COUNT", value_parser = parse_limit)]
    pub limit: Option<usize>,
}

#[derive(clap::Args, Debug)]
pub struct TextExecArgs {
    /// Which entries the ID or the chooser may pick from
    #[arg(long, value_enum, default_value = "all")]
    pub source: SourceFilter,

    /// Saved stickers in this window state (captures have no state)
    #[arg(long, value_enum, default_value = "all")]
    pub state: StateFilter,

    /// Use this entry without choosing: sticker:42 or clipboard:42
    #[arg(long, value_name = "SOURCE:ID", conflicts_with = "search")]
    pub id: Option<TextEntryId>,

    /// Pre-fill the chooser with this search
    #[arg(long, short = 's', value_name = "TEXT")]
    pub search: Option<String>,

    /// Script mode: requires --id, never opens the chooser, never prompts to unlock
    #[arg(long, requires = "id")]
    pub non_interactive: bool,

    /// The program and its arguments; {{txt}} in an argument becomes the exact text
    #[arg(
        last = true,
        required = true,
        num_args = 1..,
        value_name = "PROGRAM_ARGS",
        conflicts_with = "json"
    )]
    pub command: Vec<String>,
}

pub(crate) fn run(runtime: &Runtime, args: TextArgs, format: Format) -> anyhow::Result<()> {
    match args {
        TextArgs::List(args) => run_list(runtime, args, format),
        TextArgs::Exec(args) => run_exec(runtime, args, format),
    }
}

fn run_list(runtime: &Runtime, args: TextListArgs, format: Format) -> anyhow::Result<()> {
    let repository = runtime.text_repository()?;
    let state = args.state.to_state();
    let mut entries =
        block_on(repository.snapshot(args.source.sources(), args.search.as_deref().unwrap_or("")))?;
    entries.retain(|entry| state.is_none() || entry.state().is_none() || entry.state() == state);
    let total = entries.len();
    if let Some(limit) = args.limit {
        entries.truncate(limit);
    }

    format.emit(
        json!({"items": entries.iter().map(entry_json).collect::<Vec<_>>(), "count": entries.len(), "total": total}),
        || {
            if entries.is_empty() {
                println!("No texts match.");
                return;
            }
            let rows = entries
                .iter()
                .map(|entry| {
                    vec![
                        entry.id().to_string(),
                        if entry.is_encrypted() {
                            "locked".to_owned()
                        } else {
                            "plain".to_owned()
                        },
                        entry.state().map_or("capture", state_name).to_owned(),
                        one_line(entry.preview(), 72),
                    ]
                })
                .collect::<Vec<_>>();
            table(&["ID", "PROTECTION", "STATE", "TEXT"], &rows);
        },
    );
    Ok(())
}

/// CLI JSON uses the spoken state names (`open`/`closed`), not the storage serde spelling, so
/// every listing answers `--state` with the same vocabulary it accepts.
fn entry_json(entry: &TextEntry) -> serde_json::Value {
    let mut value = serde_json::to_value(entry).unwrap_or(serde_json::Value::Null);
    value["state"] =
        serde_json::to_value(entry.state().map(state_name)).unwrap_or(serde_json::Value::Null);
    value
}

fn run_exec(runtime: &Runtime, args: TextExecArgs, format: Format) -> anyhow::Result<()> {
    // The parser rejects `--json` together with the child program, so anything reaching here
    // speaks only for itself: the child owns stdout, stderr, and the exit status. The runtime
    // check is the safety net behind the parser.
    anyhow::ensure!(
        !format.is_json(),
        "--json cannot be combined with text exec; the child owns stdout"
    );
    // Validate the child argv before touching storage: a broken template must not create a
    // database, and the child never runs with a placeholder in the program position.
    super::text_argv::validate_template(&args.command)?;
    let repository = runtime.text_repository()?;
    super::text_execution::run(
        &repository,
        args.source.sources(),
        args.state.to_state(),
        args.id,
        args.search.as_deref().unwrap_or(""),
        &args.command,
        args.non_interactive,
    )
}

#[cfg(test)]
mod tests {
    use crate::cli::Cli;

    #[test]
    fn listing_grammar_is_valid_without_any_execution_flag() {
        for tail in [
            vec![],
            vec!["--source", "saved"],
            vec!["--source", "clipboard"],
            vec!["--source", "all", "--state", "closed"],
            vec!["-s", "rust", "-n", "5"],
            vec!["--json"],
        ] {
            let mut args = vec!["rst", "text", "list"];
            args.extend(tail);
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?}");
        }
        assert!(Cli::try_parse_from(["rst", "text", "list", "-n", "0"]).is_err());
        assert!(Cli::try_parse_from(["rst", "text", "list", "--source", "vault"]).is_err());
        assert!(Cli::try_parse_from(["rst", "text", "list", "--state", "close"]).is_err());
    }

    #[test]
    fn exec_requires_a_child_program() {
        assert!(Cli::try_parse_from(["rst", "text", "exec"]).is_err());
        assert!(Cli::try_parse_from(["rst", "text", "exec", "--"]).is_err());
        assert!(Cli::try_parse_from(["rst", "text", "exec", "--id", "sticker:1", "--"]).is_err());
        assert!(
            Cli::try_parse_from(["rst", "text", "exec", "--id", "sticker:1", "--", "tool"]).is_ok()
        );
    }

    #[test]
    fn exec_conflicts_id_search_and_parent_json_at_the_parser() {
        assert!(
            Cli::try_parse_from([
                "rst",
                "text",
                "exec",
                "--id",
                "sticker:1",
                "-s",
                "q",
                "--",
                "tool"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from(["rst", "text", "exec", "--non-interactive", "--", "tool"])
                .is_err(),
            "--non-interactive must require --id"
        );
        for args in [
            vec![
                "rst",
                "text",
                "exec",
                "--json",
                "--id",
                "sticker:1",
                "--",
                "tool",
            ],
            vec![
                "rst",
                "--json",
                "text",
                "exec",
                "--id",
                "sticker:1",
                "--",
                "tool",
            ],
        ] {
            assert!(Cli::try_parse_from(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn child_flags_after_the_separator_stay_with_the_child() {
        let cli = Cli::try_parse_from([
            "rst",
            "text",
            "exec",
            "--id",
            "clipboard:42",
            "--",
            "tool",
            "--json",
            "{{txt}}",
        ])
        .expect("child argv passes through");
        assert!(!cli.format().is_json());
    }

    #[test]
    fn ids_must_carry_their_source() {
        for id in [
            "42",
            "sticker:0",
            "clipboard:-1",
            "unknown:1",
            "sticker:nope",
        ] {
            assert!(
                Cli::try_parse_from(["rst", "text", "exec", "--id", id, "--", "tool"]).is_err(),
                "{id}"
            );
        }
    }
}
