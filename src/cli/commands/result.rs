//! `rst result` — read back what a command sticker last produced.
//!
//! A command sticker saves its output to the database every time a run finishes, which makes it a
//! usable one-way channel: create a sticker with `--no-window`, let the app run it, then collect
//! the output here without ever looking at a window.

use anyhow::bail;
use serde_json::json;

use crate::cli::runtime::Runtime;
use crate::model::content::{CommandContent, CommandResult};
use crate::model::sticker::StickerType;

use crate::cli::output::{Format, format_ts};

use super::list::type_name;

#[derive(clap::Args, Debug)]
pub struct ResultArgs {
    /// Sticker ID of a command sticker
    #[arg(value_parser = crate::cli::parse_id)]
    pub id: i64,
}

/// The declared render mode of a result, and whatever output is stored in it.
fn split(result: &CommandResult) -> (&'static str, Option<&String>) {
    match result {
        CommandResult::Text(output) => ("text", output.as_ref()),
        CommandResult::Markdown(output) => ("markdown", output.as_ref()),
        CommandResult::Html(output) => ("html", output.as_ref()),
        CommandResult::Svg(output) => ("svg", output.as_ref()),
        CommandResult::Source(output) => ("source", output.as_ref()),
    }
}

pub(crate) fn run(runtime: &Runtime, args: ResultArgs, format: Format) -> anyhow::Result<()> {
    let sticker = super::show::load(runtime, args.id)?;

    if sticker.sticker_type != StickerType::Command {
        bail!(
            "sticker {} is a {} sticker; only command stickers produce a result",
            args.id,
            type_name(sticker.sticker_type)
        );
    }

    let content: CommandContent = serde_json::from_str(&sticker.content)?;
    let (result_format, output) = split(&content.result);

    format.emit(
        json!({
            "id": sticker.id,
            "title": sticker.title,
            "command": content.command,
            "format": result_format,
            "output": output,
            "has_output": output.is_some(),
            "armed": content.started_at.is_some(),
            "started_at": content.started_at,
            "updated_at": sticker.updated_at,
        }),
        || match output {
            // Bare output on stdout, so `rst result 12 > report.md` does the obvious thing.
            Some(output) => print!("{output}"),
            None if content.started_at.is_some() => {
                eprintln!(
                    "Sticker {} is armed since {} but has no stored output. The desktop runs it; a schedule waits for its first tick.",
                    sticker.id,
                    format_ts(content.started_at.unwrap_or_default())
                );
            }
            None => eprintln!(
                "Sticker {} has no stored output. Inspect it with `rst open {}`; stopped commands need Run, selection commands need the hotkey.",
                sticker.id, sticker.id
            ),
        },
    );

    Ok(())
}
