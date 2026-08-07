use crate::model::sticker::{StickerColor, StickerDetail, StickerState, StickerType};
use crate::storage::paths::AppPaths;

use super::console::{block_on, signal_open, truncate};

pub fn run(
    app_paths: &AppPaths,
    content: Option<String>,
    title: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
    color: Option<StickerColor>,
) -> anyhow::Result<()> {
    let content = content.unwrap_or_default();

    let store = block_on(crate::storage::open_sqlite(&app_paths.db_path))?;

    let title = title.unwrap_or_else(|| {
        content
            .lines()
            .find(|l| !l.is_empty())
            .map(|l| truncate(l, 30).to_owned())
            .unwrap_or_else(|| "Markdown".to_owned())
    });

    let id = block_on(store.insert_sticker(StickerDetail {
        id: 0,
        title,
        state: StickerState::Open,
        left: 100,
        top: 100,
        width: width.unwrap_or(400),
        height: height.unwrap_or(300),
        top_most: false,
        color: color.unwrap_or(StickerColor::Yellow),
        sticker_type: StickerType::Markdown,
        content,
        created_at: 0,
        updated_at: 0,
        display_id: None,
        display_uuid: None,
        virtual_desktop_id: None,
        native_left: None,
        native_top: None,
        native_width: None,
        native_height: None,
    }))?;

    println!("Created markdown sticker (id={id})");
    signal_open(id);

    Ok(())
}
