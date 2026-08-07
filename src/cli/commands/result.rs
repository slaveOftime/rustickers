//! `rusticker result` — read back what a command sticker last produced.
//!
//! A command sticker saves its output to the database every time a run finishes, which makes it a
//! usable one-way channel: create a sticker with `--no-window`, let the app run it, then collect
//! the output here without ever looking at a window.

use anyhow::bail;
use serde_json::json;

use crate::model::content::{CommandContent, CommandResult};
use crate::model::sticker::StickerType;
use crate::storage::paths::AppPaths;

use crate::cli::output::{Format, format_ts};

use super::list::type_name;

#[derive(clap::Args, Debug)]
pub struct ResultArgs {
    /// Sticker ID of a command sticker
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

pub fn run(app_paths: &AppPaths, args: ResultArgs, format: Format) -> anyhow::Result<()> {
    let sticker = super::show::load(app_paths, args.id)?;

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
            "has_run": content.started_at.is_some(),
            "started_at": content.started_at,
            "updated_at": sticker.updated_at,
        }),
        || match output {
            // Bare output on stdout, so `rusticker result 12 > report.md` does the obvious thing.
            Some(output) => print!("{output}"),
            None if content.started_at.is_some() => {
                eprintln!(
                    "Sticker {} started at {} but has not stored any output yet.",
                    sticker.id,
                    format_ts(content.started_at.unwrap_or_default())
                );
            }
            None => eprintln!(
                "Sticker {} has not run yet. Open it with `rusticker open {}` to run it.",
                sticker.id, sticker.id
            ),
        },
    );

    Ok(())
}
