use anyhow::Context as _;
use sqlx::{
    AssertSqlSafe, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::fs;
use std::path::Path;

use crate::model::sticker::*;

impl StickerOrderBy {
    fn to_sql(self) -> &'static str {
        match self {
            Self::CreatedAsc => "created_at ASC",
            Self::CreatedDesc => "created_at DESC",
            Self::UpdatedAsc => "updated_at ASC",
            Self::UpdatedDesc => "updated_at DESC",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    pub async fn open(db_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let db_path = db_path.as_ref().to_path_buf();

        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent).context("create sqlite db parent directory")?;
        }
        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            // SQLite is single-writer; keeping this small reduces background overhead.
            .max_connections(1)
            .connect_with(options)
            .await
            .context("connect sqlite pool")?;

        // The CLI migrates too: it writes the same schema, and running it before the app has ever
        // been launched should create a usable database rather than fail with "no such table".
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("run sqlx migrations")?;

        Ok(Self { pool })
    }
}

#[async_trait::async_trait]
impl super::StickerStore for SqliteStore {
    async fn insert_sticker(&self, sticker: StickerDetail) -> anyhow::Result<i64> {
        tracing::debug!(
            sticker_type = ?sticker.sticker_type,
            title_len = sticker.title.len(),
            "Insert sticker"
        );

        let now = crate::utils::time::now_unix_millis();

        let row = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO stickers (
                title, state, left, top, width, height, color, type, content, created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11
            )
            RETURNING id
            "#,
        )
        .bind(sticker.title)
        .bind(sticker.state)
        .bind(sticker.left)
        .bind(sticker.top)
        .bind(sticker.width)
        .bind(sticker.height)
        .bind(sticker.color)
        .bind(sticker.sticker_type)
        .bind(sticker.content)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .context("insert sticker")?;

        Ok(row)
    }

    async fn delete_sticker(&self, id: i64) -> anyhow::Result<()> {
        tracing::debug!(id, "Delete sticker");
        // Foreign keys are off by default in SQLite, so the placement rows are removed explicitly.
        sqlx::query("DELETE FROM sticker_placements WHERE sticker_id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("delete sticker placements")?;
        sqlx::query("DELETE FROM stickers WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("delete sticker")?;
        Ok(())
    }

    async fn get_sticker(&self, id: i64) -> anyhow::Result<StickerDetail> {
        tracing::debug!(id, "Get sticker detail");
        let mut row = sqlx::query_as::<_, StickerDetail>(
            "SELECT id, title, state, left, top, width, height, top_most, color, type, content, created_at, updated_at, display_id, display_uuid, virtual_desktop_id, native_left, native_top, native_width, native_height, preferred_display_uuid FROM stickers WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .context("get sticker")?;

        row.placements = self.get_sticker_placements(id).await?;

        Ok(row)
    }

    async fn update_sticker_color(&self, id: i64, color: String) -> anyhow::Result<()> {
        tracing::debug!(id, color = %color, "Update sticker color");

        let now = crate::utils::time::now_unix_millis();

        sqlx::query(
            r#"
            UPDATE stickers
            SET color = ?1,
                updated_at = ?2
            WHERE id = ?3
            "#,
        )
        .bind(color)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .context("update sticker color")?;

        Ok(())
    }

    async fn update_sticker_title(&self, id: i64, title: String) -> anyhow::Result<()> {
        tracing::debug!(id, title_len = title.len(), "Update sticker title");
        let now = crate::utils::time::now_unix_millis();

        sqlx::query(
            r#"
            UPDATE stickers
            SET title = ?1,
                updated_at = ?2
            WHERE id = ?3
            "#,
        )
        .bind(title)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .context("update sticker title")?;

        Ok(())
    }

    async fn update_sticker_bounds(&self, id: i64, bounds: StickerBounds) -> anyhow::Result<()> {
        tracing::debug!(id, ?bounds, "Update sticker bounds");

        let StickerBounds {
            left,
            top,
            width,
            height,
            display_id,
            display_uuid,
            virtual_desktop_id,
            native_left,
            native_top,
            native_width,
            native_height,
        } = bounds;

        let now = crate::utils::time::now_unix_millis();

        sqlx::query(
            r#"
            UPDATE stickers
            SET left = ?1,
                top = ?2,
                width = ?3,
                height = ?4,
                display_id = ?5,
                display_uuid = ?6,
                virtual_desktop_id = ?7,
                native_left = ?8,
                native_top = ?9,
                native_width = ?10,
                native_height = ?11,
                updated_at = ?12
            WHERE id = ?13
            "#,
        )
        .bind(left)
        .bind(top)
        .bind(width)
        .bind(height)
        .bind(display_id)
        .bind(display_uuid)
        .bind(virtual_desktop_id)
        .bind(native_left)
        .bind(native_top)
        .bind(native_width)
        .bind(native_height)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .context("update sticker bounds")?;

        Ok(())
    }

    async fn update_sticker_preferred_display(
        &self,
        id: i64,
        display_uuid: Option<String>,
    ) -> anyhow::Result<()> {
        tracing::debug!(id, display_uuid, "Update sticker preferred display");

        sqlx::query("UPDATE stickers SET preferred_display_uuid = ?1 WHERE id = ?2")
            .bind(display_uuid)
            .bind(id)
            .execute(&self.pool)
            .await
            .context("update sticker preferred display")?;

        Ok(())
    }

    async fn get_sticker_placements(&self, id: i64) -> anyhow::Result<Vec<StickerPlacement>> {
        let rows = sqlx::query_as::<_, StickerPlacement>(
            r#"
            SELECT display_uuid, display_id, native_left, native_top, native_width, native_height,
                   scale_factor, updated_at
            FROM sticker_placements
            WHERE sticker_id = ?1
            ORDER BY updated_at DESC
            "#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .context("get sticker placements")?;

        Ok(rows)
    }

    async fn upsert_sticker_placement(
        &self,
        id: i64,
        placement: StickerPlacement,
        protect_display_uuid: Option<String>,
    ) -> anyhow::Result<()> {
        tracing::debug!(id, ?placement, "Upsert sticker placement");

        let mut tx = self.pool.begin().await.context("begin placement upsert")?;

        sqlx::query(
            r#"
            INSERT INTO sticker_placements (
                sticker_id, display_uuid, display_id,
                native_left, native_top, native_width, native_height,
                scale_factor, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(sticker_id, display_uuid) DO UPDATE SET
                display_id = excluded.display_id,
                native_left = excluded.native_left,
                native_top = excluded.native_top,
                native_width = excluded.native_width,
                native_height = excluded.native_height,
                scale_factor = excluded.scale_factor,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(id)
        .bind(&placement.display_uuid)
        .bind(placement.display_id)
        .bind(placement.native_left)
        .bind(placement.native_top)
        .bind(placement.native_width)
        .bind(placement.native_height)
        .bind(placement.scale_factor)
        .bind(placement.updated_at)
        .execute(&mut *tx)
        .await
        .context("upsert sticker placement")?;

        let existing = sqlx::query_as::<_, StickerPlacement>(
            r#"
            SELECT display_uuid, display_id, native_left, native_top, native_width, native_height,
                   scale_factor, updated_at
            FROM sticker_placements
            WHERE sticker_id = ?1
            "#,
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await
        .context("read sticker placements for pruning")?;

        for stale in prune_placements(
            &existing,
            protect_display_uuid.as_deref(),
            MAX_PLACEMENTS_PER_STICKER,
        ) {
            sqlx::query(
                "DELETE FROM sticker_placements WHERE sticker_id = ?1 AND display_uuid = ?2",
            )
            .bind(id)
            .bind(stale)
            .execute(&mut *tx)
            .await
            .context("prune sticker placements")?;
        }

        tx.commit().await.context("commit placement upsert")?;

        Ok(())
    }

    async fn update_sticker_content(&self, id: i64, content: String) -> anyhow::Result<()> {
        tracing::debug!(id, content_len = content.len(), "Update sticker content");

        let now = crate::utils::time::now_unix_millis();

        sqlx::query(
            r#"
            UPDATE stickers
            SET content = ?1,
                updated_at = ?2
            WHERE id = ?3
            "#,
        )
        .bind(content)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .context("update sticker content")?;

        Ok(())
    }

    async fn update_sticker_state(&self, id: i64, state: StickerState) -> anyhow::Result<()> {
        tracing::debug!(id, state = ?state, "Update sticker state");

        let now = crate::utils::time::now_unix_millis();

        sqlx::query(
            r#"
            UPDATE stickers
            SET state = ?1,
                updated_at = ?2
            WHERE id = ?3
            "#,
        )
        .bind(state)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .context("update sticker state")?;

        Ok(())
    }

    async fn update_sticker_top_most(&self, id: i64, top_most: bool) -> anyhow::Result<()> {
        tracing::debug!(id, top_most, "Update sticker top_most");

        let now = crate::utils::time::now_unix_millis();

        sqlx::query(
            r#"
            UPDATE stickers
            SET top_most = ?1,
                updated_at = ?2
            WHERE id = ?3
            "#,
        )
        .bind(top_most)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .context("update sticker top_most")?;

        Ok(())
    }

    async fn query_stickers(
        &self,
        search: Option<String>,
        order_by: StickerOrderBy,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<StickerBrief>> {
        tracing::debug!(has_search = search.as_ref().map(|s| !s.is_empty()).unwrap_or(false), order_by = ?order_by, limit, offset, "Query stickers");

        let search_pattern: Option<String> = search.map(|s| format!("%{}%", s));
        let order_sql = order_by.to_sql();

        let sql = format!(
            "SELECT id, title, state, color, type, created_at, updated_at \
             FROM stickers \
             WHERE (?1 IS NULL) OR title LIKE ?1 OR content LIKE ?1 \
             ORDER BY {} \
             LIMIT ?2 OFFSET ?3",
            order_sql
        );

        // The only interpolated fragment comes from StickerOrderBy::to_sql(),
        // which returns one of the fixed ORDER BY clauses above.
        let rows = sqlx::query_as::<_, StickerBrief>(AssertSqlSafe(sql))
            .bind(search_pattern)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
            .context("list stickers")?;

        Ok(rows)
    }

    async fn count_stickers(&self, search: Option<String>) -> anyhow::Result<i64> {
        tracing::debug!(
            has_search = search.as_ref().map(|s| !s.is_empty()).unwrap_or(false),
            "Count stickers"
        );

        let search_pattern: Option<String> = search.map(|s| format!("%{}%", s));

        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(1) FROM stickers WHERE (?1 IS NULL) OR title LIKE ?1 OR content LIKE ?1",
        )
        .bind(search_pattern)
        .fetch_one(&self.pool)
        .await
        .context("count stickers")?;

        Ok(count)
    }

    async fn get_open_sticker_ids(&self) -> anyhow::Result<Vec<i64>> {
        tracing::debug!("Get open sticker ids");

        let rows = sqlx::query_scalar::<_, i64>("SELECT id FROM stickers WHERE state = 'open'")
            .fetch_all(&self.pool)
            .await
            .context("get open sticker ids")?;

        Ok(rows)
    }

    async fn list_stickers(
        &self,
        state: Option<StickerState>,
        search: Option<String>,
    ) -> anyhow::Result<Vec<StickerListItem>> {
        tracing::debug!(
            state = ?state,
            has_search = search.as_ref().map(|s| !s.is_empty()).unwrap_or(false),
            "List stickers"
        );

        let search_pattern: Option<String> = search.map(|s| format!("%{}%", s));

        let rows = sqlx::query_as::<_, StickerListItem>(
            r#"
            SELECT id, title, state, type, left, top, width, height
            FROM stickers
            WHERE (?1 IS NULL OR state = ?1)
              AND (?2 IS NULL OR title LIKE ?2 OR content LIKE ?2)
            ORDER BY id ASC
            "#,
        )
        .bind(state)
        .bind(search_pattern)
        .fetch_all(&self.pool)
        .await
        .context("list stickers with filters")?;

        Ok(rows)
    }

    async fn touch_selection_lru(&self, id: i64, last_used_at: i64) -> anyhow::Result<()> {
        sqlx::query(
            r#"INSERT INTO selection_lru (sticker_id, last_used_at) VALUES (?1, ?2)
ON CONFLICT(sticker_id) DO UPDATE SET last_used_at = excluded.last_used_at"#,
        )
        .bind(id)
        .bind(last_used_at)
        .execute(&self.pool)
        .await
        .context("touch selection lru")?;
        Ok(())
    }

    async fn get_accept_selection_stickers(&self) -> anyhow::Result<Vec<StickerDetail>> {
        let mut rows = sqlx::query_as::<_, StickerDetail>(
            r#"SELECT s.id, s.title, s.state, s.left, s.top, s.width, s.height, s.top_most, s.color, s.type, s.content, s.created_at, s.updated_at,             s.display_id, s.display_uuid, s.virtual_desktop_id,             s.native_left, s.native_top, s.native_width, s.native_height, s.preferred_display_uuid
FROM stickers s
LEFT JOIN selection_lru l ON s.id = l.sticker_id
WHERE s.type = 'command'
  AND json_valid(s.content)
  AND COALESCE(json_extract(s.content, '$.accept_selection'), 0) = 1
ORDER BY l.last_used_at DESC NULLS LAST, s.updated_at DESC, s.id DESC"#
        )
        .fetch_all(&self.pool)
        .await
        .context("query accept_selection stickers")?;

        for row in rows.iter_mut() {
            row.placements = self.get_sticker_placements(row.id).await?;
        }

        Ok(rows)
    }

    async fn get_scheduled_command_stickers(&self) -> anyhow::Result<Vec<ScheduledCommand>> {
        // `started_at` is the arming flag and `scheduler.Cron` the expression; a sticker needs
        // both to be due for a background run. `state` is deliberately not filtered on, that is
        // the whole point: a closed sticker keeps its schedule.
        sqlx::query_as::<_, ScheduledCommand>(
            r#"SELECT id, title, content
FROM stickers
WHERE type = 'command'
  AND json_valid(content)
  AND json_extract(content, '$.started_at') IS NOT NULL
  AND COALESCE(json_extract(content, '$.scheduler.Cron'), '') <> ''
ORDER BY id"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("query scheduled command stickers")
    }
}
