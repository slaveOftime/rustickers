//! Window visibility is persisted separately from execution/schedule state.
use crate::{
    cli::{
        output::Format,
        runtime::{Delivery, Runtime, block_on},
    },
    model::sticker::StickerState,
};
use serde_json::json;

#[derive(clap::Args, Debug)]
pub struct WindowArgs {
    /// Positive sticker ID from `rst list`
    #[arg(value_parser = crate::cli::parse_id)]
    pub id: i64,
}

pub(crate) fn open(runtime: &Runtime, args: WindowArgs, format: Format) -> anyhow::Result<()> {
    run(runtime, args.id, StickerState::Open, format)
}
pub(crate) fn close(runtime: &Runtime, args: WindowArgs, format: Format) -> anyhow::Result<()> {
    run(runtime, args.id, StickerState::Close, format)
}
fn run(runtime: &Runtime, id: i64, state: StickerState, format: Format) -> anyhow::Result<()> {
    let sticker = super::show::load(runtime, id)?;
    let already = sticker.state == state;
    if !already {
        block_on(runtime.store()?.update_sticker_state(id, state))?;
    }
    let delivery = match state {
        StickerState::Open => runtime.open_sticker(id),
        StickerState::Close => runtime.close_sticker(id),
    };
    let state = super::list::state_name(state);
    format.emit(
        json!({"id":id,"state":state,"changed":!already,"desktop_notification":delivery.as_str()}),
        || {
            println!("Sticker {id} marked {state}.");
            match delivery {
                Delivery::Sent => println!("Window request sent to Rustickers."),
                Delivery::Offline => println!("Saved offline; no desktop request was sent."),
                _ => println!("Desktop unavailable; visibility applies on next launch."),
            }
        },
    );
    Ok(())
}
