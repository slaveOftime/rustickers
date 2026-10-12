//! `rst delete` — remove a sticker for good.

use anyhow::bail;
use serde_json::json;
use std::io::{IsTerminal as _, Write as _};

use crate::cli::output::{Format, one_line};
use crate::cli::runtime::{Runtime, block_on};

use super::list::type_name;

#[derive(clap::Args, Debug)]
pub struct DeleteArgs {
    /// Sticker ID, as listed by `rst list`
    #[arg(value_parser = crate::cli::parse_id)]
    pub id: i64,

    /// Delete without asking for confirmation
    #[arg(long, short = 'y')]
    pub yes: bool,
}

pub(crate) fn run(runtime: &Runtime, args: DeleteArgs, format: Format) -> anyhow::Result<()> {
    let store = runtime.store()?;
    let sticker = super::show::load(runtime, args.id)?;

    if !args.yes {
        // There is nobody at a terminal to answer a prompt in JSON mode, so ask for the flag
        // instead of hanging on a read that will never be answered.
        if format.is_json() || !std::io::stdin().is_terminal() {
            bail!("refusing to delete without --yes outside an interactive terminal");
        }
        eprint!(
            "Delete {} sticker {} ({})? [y/N] ",
            type_name(sticker.sticker_type),
            sticker.id,
            one_line(&sticker.title, 40)
        );
        std::io::stderr().flush()?;

        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Left alone.");
            return Ok(());
        }
    }

    // Close it first so the app is not left holding a window for a sticker that no longer exists.
    block_on(store.delete_sticker(args.id))?;
    let delivery = runtime.close_sticker(args.id);

    format.emit(
        json!({
            "id": args.id,
            "deleted": true,
            "title": sticker.title,
            "type": type_name(sticker.sticker_type),
            "desktop_notification": delivery.as_str(),
        }),
        || println!("Deleted sticker {}.", args.id),
    );

    Ok(())
}
