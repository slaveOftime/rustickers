pub mod paths;
pub mod sqlite;

use std::path::Path;
use std::sync::Arc;

use crate::model::sticker::*;

#[allow(dead_code)]
#[async_trait::async_trait]
pub trait StickerStore: Send + Sync {
    async fn insert_sticker(&self, sticker: StickerDetail) -> anyhow::Result<i64>;
    async fn delete_sticker(&self, id: i64) -> anyhow::Result<()>;
    async fn get_sticker(&self, id: i64) -> anyhow::Result<StickerDetail>;

    async fn update_sticker_color(&self, id: i64, color: String) -> anyhow::Result<()>;
    async fn update_sticker_title(&self, id: i64, title: String) -> anyhow::Result<()>;
    async fn update_sticker_bounds(
        &self,
        id: i64,
        left: i32,
        top: i32,
        width: i32,
        height: i32,
        display_id: Option<i64>,
        display_uuid: Option<String>,
        virtual_desktop_id: Option<String>,
        native_left: Option<i32>,
        native_top: Option<i32>,
        native_width: Option<i32>,
        native_height: Option<i32>,
    ) -> anyhow::Result<()>;
    async fn update_sticker_content(&self, id: i64, content: String) -> anyhow::Result<()>;
    async fn update_sticker_state(&self, id: i64, state: StickerState) -> anyhow::Result<()>;
    #[allow(dead_code)]
    async fn update_sticker_top_most(&self, id: i64, top_most: bool) -> anyhow::Result<()>;

    async fn query_stickers(
        &self,
        search: Option<String>,
        order_by: StickerOrderBy,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<StickerBrief>>;
    async fn count_stickers(&self, search: Option<String>) -> anyhow::Result<i64>;
    async fn get_open_sticker_ids(&self) -> anyhow::Result<Vec<i64>>;
    async fn list_stickers(
        &self,
        state: Option<StickerState>,
        search: Option<String>,
    ) -> anyhow::Result<Vec<StickerListItem>>;

    /// Touch the selection LRU timestamp for a command sticker.
    async fn touch_selection_lru(&self, id: i64, last_used_at: i64) -> anyhow::Result<()>;

    /// Get command stickers with accept_selection=true, ordered by LRU (most recently used first).
    async fn get_accept_selection_stickers(&self) -> anyhow::Result<Vec<StickerDetail>>;
}

pub type ArcStickerStore = Arc<dyn StickerStore>;

pub async fn open_sqlite(db_path: impl AsRef<Path>) -> anyhow::Result<ArcStickerStore> {
    let store = sqlite::SqliteStore::open(db_path).await?;
    Ok(Arc::new(store))
}
