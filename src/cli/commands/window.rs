//! `rusticker open` and `rusticker close` — show or hide a sticker's window.
//!
//! Both write the new state to the database first and only then tell the app, so the request
//! survives the app not running: the sticker comes up in the right state on the next launch.

use serde_json::json;

use crate::model::sticker::StickerState;
use crate::storage::paths::AppPaths;

use crate::cli::output::Format;
use crate::cli::runtime::{self, Delivery, block_on, open_store};

#[derive(clap::Args, Debug)]
pub struct WindowArgs {
    /// Sticker ID, as listed by `rusticker list`
    pub id: i64,
}

pub fn open(app_paths: &AppPaths, args: WindowArgs, format: Format) -> anyhow::Result<()> {
    run(app_paths, args.id, StickerState::Open, format)
}

pub fn close(app_paths: &AppPaths, args: WindowArgs, format: Format) -> anyhow::Result<()> {
    run(app_paths, args.id, StickerState::Close, format)
}

fn run(app_paths: &AppPaths, id: i64, state: StickerState, format: Format) -> anyhow::Result<()> {
    let store = open_store(app_paths)?;

    // Reading it first turns a typo into "no sticker with id 99" instead of a silent success.
    let sticker =
        block_on(store.get_sticker(id)).map_err(|_| anyhow::anyhow!("no sticker with id {id}"))?;

    let already = sticker.state == state;
    if !already {
        block_on(store.update_sticker_state(id, state))?;
    }

    let delivery = match state {
        StickerState::Open => runtime::open_sticker(id),
        StickerState::Close => runtime::close_sticker(id),
    };

    let verb = match state {
        StickerState::Open => "open",
        StickerState::Close => "closed",
    };

    format.emit(
        json!({
            "id": id,
            "state": super::list::state_name(state),
            "changed": !already,
            "app_running": delivery.delivered(),
        }),
        || match delivery {
            Delivery::Delivered => println!("Sticker {id} is now {verb}."),
            Delivery::AppNotRunning => println!(
                "Rustickers is not running — sticker {id} is marked {verb} and will be {verb} on \
                 next launch."
            ),
        },
    );

    Ok(())
}
