use super::ListState;
use super::console::{block_on, truncate};
use crate::model::sticker::StickerState;
use crate::storage::paths::AppPaths;

pub fn run(app_paths: &AppPaths, state: ListState, search: Option<String>) -> anyhow::Result<()> {
    let store = block_on(crate::storage::open_sqlite(&app_paths.db_path))?;
    let state_filter = match state {
        ListState::Open => Some(StickerState::Open),
        ListState::Close => Some(StickerState::Close),
        ListState::All => None,
    };

    let stickers = block_on(store.list_stickers(state_filter, search))?;

    if stickers.is_empty() {
        if matches!(state, ListState::Open) {
            println!("No open stickers.");
        } else {
            println!("No stickers found.");
        }
        return Ok(());
    }

    println!(
        "{:<6}  {:<30}  {:<10}  {:<12}  {}",
        "ID", "TITLE", "TYPE", "POSITION", "SIZE"
    );
    println!("{}", "-".repeat(70));

    for sticker in stickers {
        let title = truncate(&sticker.title, 30);
        let type_str = format!("{:?}", sticker.sticker_type);
        let pos = format!("{},{}", sticker.left, sticker.top);
        let size = format!("{}x{}", sticker.width, sticker.height);
        println!(
            "{:<6}  {:<30}  {:<10}  {:<12}  {}",
            sticker.id, title, type_str, pos, size
        );
    }

    Ok(())
}
