use anyhow::Context as _;
use sqlx::{
    SqlitePool,
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
        sqlx::query("DELETE FROM stickers WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("delete sticker")?;
        Ok(())
    }

    async fn get_sticker(&self, id: i64) -> anyhow::Result<StickerDetail> {
        tracing::debug!(id, "Get sticker detail");
        let row = sqlx::query_as::<_, StickerDetail>(
            "SELECT id, title, state, left, top, width, height, top_most, color, type, content, created_at, updated_at FROM stickers WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .context("get sticker")?;

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

    async fn update_sticker_bounds(
        &self,
        id: i64,
        left: i32,
        top: i32,
        width: i32,
        height: i32,
    ) -> anyhow::Result<()> {
        tracing::debug!(id, left, top, width, height, "Update sticker bounds");

        let now = crate::utils::time::now_unix_millis();

        sqlx::query(
            r#"
            UPDATE stickers
            SET left = ?1,
                top = ?2,
                width = ?3,
                height = ?4,
                updated_at = ?5
            WHERE id = ?6
            "#,
        )
        .bind(left)
        .bind(top)
        .bind(width)
        .bind(height)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .context("update sticker bounds")?;

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

        let rows = sqlx::query_as::<_, StickerBrief>(&sql)
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
}
