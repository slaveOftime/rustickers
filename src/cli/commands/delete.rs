//! `rusticker delete` — remove a sticker for good.

use anyhow::bail;
use serde_json::json;
use std::io::Write as _;

use crate::storage::paths::AppPaths;

use crate::cli::output::{Format, one_line};
use crate::cli::runtime::{self, block_on, open_store};

use super::list::type_name;

#[derive(clap::Args, Debug)]
pub struct DeleteArgs {
    /// Sticker ID, as listed by `rusticker list`
    pub id: i64,

    /// Delete without asking for confirmation
    #[arg(long, short = 'y')]
    pub yes: bool,
}

pub fn run(app_paths: &AppPaths, args: DeleteArgs, format: Format) -> anyhow::Result<()> {
    let store = open_store(app_paths)?;
    let sticker = block_on(store.get_sticker(args.id))
        .map_err(|_| anyhow::anyhow!("no sticker with id {}", args.id))?;

    if !args.yes {
        // There is nobody at a terminal to answer a prompt in JSON mode, so ask for the flag
        // instead of hanging on a read that will never be answered.
        if format.is_json() {
            bail!("refusing to delete without --yes");
        }
        print!(
            "Delete {} sticker {} ({})? [y/N] ",
            type_name(sticker.sticker_type),
            sticker.id,
            one_line(&sticker.title, 40)
        );
        std::io::stdout().flush()?;

        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Left alone.");
            return Ok(());
        }
    }

    // Close it first so the app is not left holding a window for a sticker that no longer exists.
    let delivery = runtime::close_sticker(args.id);
    block_on(store.delete_sticker(args.id))?;

    format.emit(
        json!({
            "id": args.id,
            "deleted": true,
            "title": sticker.title,
            "type": type_name(sticker.sticker_type),
            "app_running": delivery.delivered(),
        }),
        || println!("Deleted sticker {}.", args.id),
    );

    Ok(())
}
