use super::{
    TextEntry, TextEntryId, TextSources, compare_by_recency,
    protection::{self, MasterKey},
};
use crate::{clipboard::ClipboardSettings, storage::sqlite::SqliteStore};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use zeroize::Zeroizing;

/// Outcome of [`TextRepository::capture`]: either a fresh `clipboard_captures`
/// row was inserted or refreshed, or an existing saved sticker absorbed the
/// new copy's recency and nothing was stored in the captures table at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// A `clipboard_captures` row ID (created or refreshed) carries this
    /// capture. The row already exists exactly once in the captures table.
    Stored(i64),
    /// A saved markdown sticker with a matching plaintext digest was found.
    /// Its recency metadata was advanced and no capture row was inserted;
    /// the carried value is the saved sticker ID.
    RefreshedSticker(i64),
}
impl CaptureOutcome {
    /// The capture row id when one was stored/refreshed, or `None` when the
    /// new copy was deduplicated against an existing saved sticker.
    pub fn capture_id(self) -> Option<i64> {
        match self {
            Self::Stored(id) => Some(id),
            Self::RefreshedSticker(_) => None,
        }
    }
    /// The saved-sticker id when the new copy was absorbed into an existing
    /// sticker, or `None` when a fresh capture row was created.
    pub fn sticker_id(self) -> Option<i64> {
        match self {
            Self::Stored(_) => None,
            Self::RefreshedSticker(id) => Some(id),
        }
    }
}

/// Composes text and capture operations on the sticker store's canonical pool.
#[derive(Clone)]
pub struct TextRepository {
    pool: SqlitePool,
}
impl TextRepository {
    pub fn new(store: &SqliteStore) -> Self {
        Self { pool: store.pool() }
    }
    pub async fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let path = path.as_ref();
        let repository = Self::new(&SqliteStore::open(path).await?);
        repository
            .import_legacy(path.with_file_name("clipboard.db"))
            .await?;
        Ok(repository)
    }

    /// One-shot read-only import. Never decrypts or mutates the legacy database.
    pub async fn import_legacy(&self, path: impl AsRef<std::path::Path>) -> Result<()> {
        if !path.as_ref().is_file() {
            return Ok(());
        }
        let imported: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM text_imports WHERE source='legacy-clipboard'")
                .fetch_one(&self.pool)
                .await?;
        if imported != 0 {
            return Ok(());
        }
        let legacy = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(path.as_ref())
                    .read_only(true),
            )
            .await
            .context("open legacy clipboard read-only")?;
        let rows = sqlx::query(
            "SELECT content,created_at,last_copied_at,last_used_at FROM clipboard_items",
        )
        .fetch_all(&legacy)
        .await
        .context("read legacy raw clipboard text")?;
        let legacy_settings = sqlx::query("SELECT key,value FROM clipboard_settings")
            .fetch_all(&legacy)
            .await?;
        legacy.close().await;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let imported: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM text_imports WHERE source='legacy-clipboard'")
                .fetch_one(&mut *tx)
                .await?;
        if imported != 0 {
            return Ok(());
        }
        // Import collection preferences once, without importing any protection metadata.
        for row in legacy_settings {
            let key: String = row.try_get("key")?;
            let value: String = row.try_get("value")?;
            let valid = match key.as_str() {
                "normal_count_limit" | "normal_byte_limit" => {
                    value.parse::<i64>().is_ok_and(|n| n >= 0)
                }
                "capture_enabled" => value == "0" || value == "1",
                _ => false,
            };
            if valid {
                sqlx::query("UPDATE clipboard_settings SET value=? WHERE key=?")
                    .bind(value)
                    .bind(key)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        backfill_plaintext_digests(&mut tx).await?;
        let mut copied = 0i64;
        let mut skipped = 0i64;
        // Include obsolete rows already in the canonical DB; only raw content is eligible.
        let mut all = sqlx::query(
            "SELECT content,created_at,last_copied_at,last_used_at FROM clipboard_items",
        )
        .fetch_all(&mut *tx)
        .await?;
        all.extend(rows);
        for row in all {
            let text: String = row.try_get("content")?;
            if text.trim().is_empty()
                || text.contains('\0')
                || text.len() > 1024 * 1024
                || protection::is_encrypted(&text)
                || protection::is_unsupported(&text)
            {
                skipped += 1;
                continue;
            }
            let fingerprint = digest(&text);
            let saved = matching_sticker(&mut tx, &fingerprint).await?;
            if let Some(id) = saved {
                let activity = row
                    .try_get::<i64, _>("created_at")?
                    .max(row.try_get("last_copied_at")?)
                    .max(row.try_get::<Option<i64>, _>("last_used_at")?.unwrap_or(0));
                sqlx::query("UPDATE stickers SET text_last_used_at=MAX(COALESCE(text_last_used_at,0),?) WHERE id=?")
                    .bind(activity).bind(id).execute(&mut *tx).await?;
                absorb_captures(&mut tx, id, &text).await?;
                continue;
            }
            let existing: Option<i64> =
                sqlx::query_scalar("SELECT id FROM clipboard_captures WHERE text=? LIMIT 1")
                    .bind(&text)
                    .fetch_optional(&mut *tx)
                    .await?;
            let copied_at: i64 = row.try_get("last_copied_at")?;
            let used_at: Option<i64> = row.try_get("last_used_at")?;
            if let Some(id) = existing {
                sqlx::query("UPDATE clipboard_captures SET last_copied_at=MAX(last_copied_at,?),last_used_at=MAX(COALESCE(last_used_at,0),COALESCE(?,0)) WHERE id=?")
                    .bind(copied_at).bind(used_at).bind(id).execute(&mut *tx).await?;
            } else {
                sqlx::query("INSERT INTO clipboard_captures(text,text_digest,byte_size,created_at,last_copied_at,last_used_at) VALUES(?,?,?,?,?,?)")
                    .bind(&text).bind(&fingerprint)
                    .bind(text.len() as i64).bind(row.try_get::<i64,_>("created_at")?)
                    .bind(copied_at).bind(used_at).execute(&mut *tx).await?;
                copied += 1;
            }
        }
        let settings = settings_in(&mut tx).await?;
        prune(&mut tx, &settings).await?;
        sqlx::query("INSERT INTO text_imports(source,imported_at,imported_count,skipped_count) VALUES('legacy-clipboard',?,?,?)")
            .bind(crate::utils::time::now_unix_millis()).bind(copied).bind(skipped).execute(&mut *tx).await?;
        tx.commit().await?;
        tracing::info!(
            imported_count = copied,
            skipped_count = skipped,
            "Imported legacy raw clipboard text"
        );
        Ok(())
    }
    /// Content-free cross-process invalidation for desktop list refresh.
    pub async fn data_version(&self) -> Result<i64> {
        // A single-connection pool observes both its own mutations and external commits.
        let mut connection = self.pool.acquire().await?;
        let external: i64 = sqlx::query_scalar("PRAGMA data_version")
            .fetch_one(&mut *connection)
            .await?;
        let local: i64 = sqlx::query_scalar("SELECT total_changes()")
            .fetch_one(&mut *connection)
            .await?;
        Ok(external.wrapping_mul(1_000_000_007).wrapping_add(local))
    }

    pub async fn master_key(&self) -> Result<Option<MasterKey>> {
        let value: Option<String> =
            sqlx::query_scalar("SELECT configuration FROM text_master_key WHERE singleton = 1")
                .fetch_optional(&self.pool)
                .await?;
        value.as_deref().map(MasterKey::parse).transpose()
    }
    pub async fn set_master_password(&self, password: &str) -> Result<()> {
        let key = MasterKey::create(password)?.serialize()?;
        sqlx::query("INSERT INTO text_master_key(singleton, configuration) VALUES(1, ?)")
            .bind(key)
            .execute(&self.pool)
            .await
            .context("master password already configured or could not be saved")?;
        Ok(())
    }
    pub async fn settings(&self) -> Result<ClipboardSettings> {
        let mut settings = ClipboardSettings::default();
        for row in sqlx::query("SELECT key, value FROM clipboard_settings")
            .fetch_all(&self.pool)
            .await?
        {
            let key: String = row.try_get("key")?;
            let value: String = row.try_get("value")?;
            match key.as_str() {
                "normal_count_limit" => settings.normal_count_limit = value.parse()?,
                "normal_byte_limit" => settings.normal_byte_limit = value.parse()?,
                "capture_enabled" => settings.capture_enabled = value == "1",
                _ => {}
            }
        }
        Ok(settings)
    }
    pub async fn update_settings(&self, settings: &ClipboardSettings) -> Result<()> {
        ensure!(
            settings.normal_count_limit >= 0 && settings.normal_byte_limit >= 0,
            "capture limits cannot be negative"
        );
        let mut tx = self.pool.begin().await?;
        for (key, value) in [
            (
                "normal_count_limit",
                settings.normal_count_limit.to_string(),
            ),
            ("normal_byte_limit", settings.normal_byte_limit.to_string()),
            (
                "capture_enabled",
                if settings.capture_enabled { "1" } else { "0" }.to_owned(),
            ),
        ] {
            sqlx::query("INSERT INTO clipboard_settings(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
                .bind(key).bind(value).execute(&mut *tx).await?;
        }
        prune(&mut tx, settings).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn capture(&self, text: &str, now: i64) -> Result<CaptureOutcome> {
        ensure!(
            !text.trim().is_empty() && !text.contains('\0'),
            "capture must be nonempty text without NUL"
        );
        ensure!(text.len() <= 1024 * 1024, "capture exceeds one MiB");
        let digest = digest(text);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let settings = settings_in(&mut tx).await?;
        // Cross-format dedup: if a saved sticker (plaintext or encrypted) already
        // matches this content by digest, refresh its recency instead of creating
        // a duplicate capture. The sticker carries the same plaintext digest even
        // when its `content` is a sealed envelope, so the same lookup handles both.
        backfill_plaintext_digests(&mut tx).await?;
        let existing_sticker = matching_sticker(&mut tx, &digest).await?;
        if let Some(sticker) = existing_sticker {
            let result = sqlx::query(
                "UPDATE stickers SET text_last_used_at=MAX(COALESCE(text_last_used_at,0),?) WHERE id=? AND type='markdown'",
            )
            .bind(now)
            .bind(sticker)
            .execute(&mut *tx)
            .await?;
            ensure!(
                result.rows_affected() == 1,
                "matched sticker disappeared before recency refresh"
            );
            absorb_captures(&mut tx, sticker, text).await?;
            tx.commit().await?;
            return Ok(CaptureOutcome::RefreshedSticker(sticker));
        }
        let existing_capture: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM clipboard_captures WHERE text_digest=? AND text=? LIMIT 1",
        )
        .bind(&digest)
        .bind(text)
        .fetch_optional(&mut *tx)
        .await?;
        let id = if let Some(id) = existing_capture {
            sqlx::query(
                "UPDATE clipboard_captures SET last_copied_at=MAX(last_copied_at,?) WHERE id=?",
            )
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
            id
        } else {
            sqlx::query_scalar("INSERT INTO clipboard_captures(text,text_digest,byte_size,created_at,last_copied_at) VALUES(?,?,?,?,?) RETURNING id")
                .bind(text).bind(&digest).bind(text.len() as i64).bind(now).bind(now).fetch_one(&mut *tx).await?
        };
        prune(&mut tx, &settings).await?;
        tx.commit().await?;
        Ok(CaptureOutcome::Stored(id))
    }
    pub async fn snapshot(&self, sources: TextSources, query: &str) -> Result<Vec<TextEntry>> {
        let mut entries = Vec::new();
        if sources.includes(TextEntryId::Sticker(1)) {
            for row in sqlx::query("SELECT id,title,state,content,created_at,COALESCE(text_modified_at,created_at) AS modified,COALESCE(text_last_used_at,0) AS used,text_revision,COALESCE(text_digest,'') AS digest FROM stickers WHERE type='markdown' ORDER BY id ASC")
                .fetch_all(&self.pool).await? {
                let content: String = row.try_get("content")?;
                if protection::is_unsupported(&content) { continue; }
                let encrypted = protection::is_encrypted(&content);
                let title: String = row.try_get("title")?; let created_at: i64 = row.try_get("created_at")?;
                let mut stored_digest: String = row.try_get("digest")?;
                // Read-only fallback for legacy rows; persistent backfill runs at open/capture.
                if stored_digest.is_empty() && !encrypted {
                    stored_digest = digest(&content);
                }
                entries.push(TextEntry { id: TextEntryId::Sticker(row.try_get("id")?),
                    preview: preview(if encrypted { &title } else { &content }), title, encrypted,
                    state: Some(row.try_get("state")?), created_at,
                    activity_at: created_at.max(row.try_get("modified")?).max(row.try_get("used")?),
                    revision: row.try_get("text_revision")?,
                    digest: stored_digest, content });
            }
        }
        if sources.includes(TextEntryId::Clipboard(1)) {
            for row in
                sqlx::query("SELECT id,text,created_at,last_copied_at,text_digest,COALESCE(last_used_at,0) AS used FROM clipboard_captures")
                    .fetch_all(&self.pool)
                    .await?
            {
                let content: String = row.try_get("text")?;
                let created_at: i64 = row.try_get("created_at")?;
                let activity_at = created_at
                    .max(row.try_get("last_copied_at")?)
                    .max(row.try_get("used")?);
                let capture_digest: String = row.try_get("text_digest")?;
                // Projection dedupe only: a capture can refresh the representative
                // sticker's display activity. Digest matching also folds captures
                // whose plaintext matches an already-locked sticker.
                if !capture_digest.is_empty()
                    && let Some(sticker) = entries
                        .iter_mut()
                        .find(|e| matches!(e.id, TextEntryId::Sticker(_)) && !e.digest.is_empty() && e.digest == capture_digest)
                {
                    sticker.activity_at = sticker.activity_at.max(activity_at);
                    continue;
                }
                entries.push(TextEntry {
                    id: TextEntryId::Clipboard(row.try_get("id")?),
                    title: String::new(),
                    preview: preview(&content),
                    encrypted: false,
                    state: None,
                    created_at,
                    activity_at,
                    revision: 0,
                    digest: capture_digest,
                    content,
                });
            }
        }
        entries.retain(|e| e.matches(query));
        entries.sort_by(compare_by_recency);
        Ok(entries)
    }
    pub async fn get(&self, id: TextEntryId) -> Result<TextEntry> {
        // Query separately so projection deduplication never makes direct identity disappear.
        let sources = TextSources::new(
            matches!(id, TextEntryId::Sticker(_)),
            matches!(id, TextEntryId::Clipboard(_)),
        );
        self.snapshot(sources, "")
            .await?
            .into_iter()
            .find(|e| e.id == id)
            .context("text no longer available")
    }
    pub async fn resolve(
        &self,
        expected: &TextEntry,
        password: Option<&str>,
    ) -> Result<Zeroizing<String>> {
        let current = self.get(expected.id).await?;
        ensure!(
            current.revision == expected.revision && current.content == expected.content,
            "text changed; select it again"
        );
        let text = if current.encrypted {
            let TextEntryId::Sticker(id) = current.id else {
                unreachable!()
            };
            self.master_key()
                .await?
                .context("master password not configured")?
                .open(
                    id,
                    &current.content,
                    password.context("master password required")?,
                )?
        } else {
            Zeroizing::new(current.content.clone())
        };
        let after = self.get(expected.id).await?;
        ensure!(
            after.revision == current.revision && after.content == current.content,
            "text changed during authorization"
        );
        Ok(text)
    }
    pub async fn promote(&self, expected: &TextEntry, encrypted: bool) -> Result<i64> {
        self.promote_titled(expected, encrypted, None).await
    }
    pub async fn promote_titled(
        &self,
        expected: &TextEntry,
        encrypted: bool,
        title: Option<&str>,
    ) -> Result<i64> {
        if let Some(title) = title {
            ensure!(!title.trim().is_empty(), "title is required");
        }
        let TextEntryId::Clipboard(capture_id) = expected.id else {
            anyhow::bail!("only captures can be promoted")
        };
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let content: String = sqlx::query_scalar("SELECT text FROM clipboard_captures WHERE id=?")
            .bind(capture_id)
            .fetch_optional(&mut *tx)
            .await?
            .context("capture no longer available")?;
        ensure!(content == expected.content, "capture changed");
        let now = crate::utils::time::now_unix_millis();
        let id: i64 = sqlx::query_scalar("INSERT INTO stickers(title,state,left,top,width,height,color,type,content,created_at,updated_at,text_modified_at) VALUES('','close',100,100,400,300,'yellow','markdown','',?,?,?) RETURNING id")
            .bind(now).bind(now).bind(now).fetch_one(&mut *tx).await?;
        let saved = if encrypted {
            key_in(&mut tx)
                .await?
                .context("set master password first")?
                .seal(id, &content)?
        } else {
            content.clone()
        };
        sqlx::query("UPDATE stickers SET title=?,content=?,text_digest=? WHERE id=?")
            .bind(title.map(str::to_owned).unwrap_or_else(|| {
                if encrypted {
                    "Locked text".to_owned()
                } else {
                    preview(&content)
                }
            }))
            .bind(saved)
            .bind(digest(&content))
            .bind(id)
            .execute(&mut *tx)
            .await?;
        absorb_captures(&mut tx, id, &content).await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn save_text(&self, expected: &TextEntry, text: &str) -> Result<()> {
        let TextEntryId::Sticker(id) = expected.id else {
            anyhow::bail!("promote capture before editing")
        };
        ensure!(
            !protection::is_unsupported(text),
            "unsupported text envelope"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let content = if expected.encrypted {
            key_in(&mut tx)
                .await?
                .context("master password not configured")?
                .seal(id, text)?
        } else {
            ensure!(
                !protection::is_encrypted(text),
                "cannot save an encryption envelope as plaintext"
            );
            text.to_owned()
        };
        let result = sqlx::query("UPDATE stickers SET content=?,title=CASE WHEN ? OR title != ? THEN title ELSE ? END,text_digest=?,text_revision=text_revision+1,text_modified_at=?,updated_at=? WHERE id=? AND type='markdown' AND text_revision=? AND content=?")
            .bind(content)
            .bind(expected.encrypted)
            .bind(preview(&expected.content))
            .bind(if expected.encrypted { String::new() } else { preview(text) })
            .bind(digest(text))
            .bind(crate::utils::time::now_unix_millis())
            .bind(crate::utils::time::now_unix_millis())
            .bind(id)
            .bind(expected.revision)
            .bind(&expected.content)
            .execute(&mut *tx)
            .await?;
        ensure!(
            result.rows_affected() == 1,
            "text changed; reload before saving"
        );
        absorb_captures(&mut tx, id, text).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn set_encryption(
        &self,
        expected: &TextEntry,
        enabled: bool,
        password: Option<&str>,
    ) -> Result<()> {
        self.set_encryption_titled(expected, enabled, password, None)
            .await
    }
    pub async fn set_encryption_titled(
        &self,
        expected: &TextEntry,
        enabled: bool,
        password: Option<&str>,
        title: Option<&str>,
    ) -> Result<()> {
        if let Some(title) = title {
            ensure!(!title.trim().is_empty(), "title is required");
        }
        let TextEntryId::Sticker(id) = expected.id else {
            anyhow::bail!("promote capture to encrypt it")
        };
        ensure!(
            enabled != expected.encrypted,
            "text already has requested protection"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let key = key_in(&mut tx)
            .await?
            .context("set master password first")?;
        let plaintext = if enabled {
            Zeroizing::new(expected.content.clone())
        } else {
            key.open(
                id,
                &expected.content,
                password.context("master password required")?,
            )?
        };
        let replacement = if enabled {
            key.seal(id, &plaintext)?
        } else {
            plaintext.to_string()
        };
        let plaintext_digest = digest(&plaintext);
        let result = sqlx::query("UPDATE stickers SET content=?,title=?,text_digest=?,text_revision=text_revision+1 WHERE id=? AND type='markdown' AND text_revision=? AND content=?")
            .bind(replacement).bind(title.map(str::to_owned).unwrap_or_else(|| if enabled { "Locked text".to_owned() } else { expected.title.clone() }))
            .bind(plaintext_digest)
            .bind(id).bind(expected.revision).bind(&expected.content).execute(&mut *tx).await?;
        ensure!(
            result.rows_affected() == 1,
            "text changed; reload before changing protection"
        );
        absorb_captures(&mut tx, id, &plaintext).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn update_title(&self, id: TextEntryId, title: &str) -> Result<()> {
        ensure!(!title.trim().is_empty(), "title is required");
        let TextEntryId::Sticker(id) = id else {
            anyhow::bail!("save capture before renaming");
        };
        let result =
            sqlx::query("UPDATE stickers SET title=?,updated_at=? WHERE id=? AND type='markdown'")
                .bind(title)
                .bind(crate::utils::time::now_unix_millis())
                .bind(id)
                .execute(&self.pool)
                .await?;
        ensure!(result.rows_affected() == 1, "text no longer available");
        Ok(())
    }
    pub async fn change_master_password(&self, old: &str, new: &str) -> Result<()> {
        // Single writer transaction prevents concurrent membership changes and stale generations.
        // Bounded one-payload-at-a-time plaintext; no plaintext staging on disk.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let previous = key_in(&mut tx)
            .await?
            .context("master password not configured")?;
        let identity = previous.authorize(old)?;
        let next = MasterKey::create(new)?;
        let rows = sqlx::query("SELECT id,content FROM stickers WHERE type='markdown' AND content GLOB 'RUSTICKERS_TEXT_*'")
            .fetch_all(&mut *tx).await?;
        for row in rows {
            let id: i64 = row.try_get("id")?;
            let envelope: String = row.try_get("content")?;
            let plaintext = previous.open_authorized(id, &envelope, &identity)?;
            let replacement = next.seal(id, &plaintext)?;
            sqlx::query("UPDATE stickers SET content=?,text_digest=?,text_revision=text_revision+1 WHERE id=?")
                .bind(replacement)
                .bind(digest(&plaintext))
                .bind(id)
                .execute(&mut *tx)
                .await?;
            absorb_captures(&mut tx, id, &plaintext).await?;
        }
        sqlx::query("UPDATE text_master_key SET configuration=? WHERE singleton=1")
            .bind(next.serialize()?)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn mark_used(&self, id: TextEntryId, now: i64) -> Result<()> {
        let (query, number) = match id {
            TextEntryId::Sticker(n) => (
                "UPDATE stickers SET text_last_used_at=MAX(COALESCE(text_last_used_at,0),?) WHERE id=? AND type='markdown'",
                n,
            ),
            TextEntryId::Clipboard(n) => (
                "UPDATE clipboard_captures SET last_used_at=MAX(COALESCE(last_used_at,0),?) WHERE id=?",
                n,
            ),
        };
        sqlx::query(query)
            .bind(now)
            .bind(number)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn delete(&self, id: TextEntryId) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        match id {
            TextEntryId::Clipboard(n) => {
                sqlx::query("DELETE FROM clipboard_captures WHERE id=?")
                    .bind(n)
                    .execute(&mut *tx)
                    .await?;
            }
            TextEntryId::Sticker(n) => {
                sqlx::query("DELETE FROM sticker_placements WHERE sticker_id=? AND EXISTS(SELECT 1 FROM stickers WHERE id=? AND type='markdown')")
                    .bind(n).bind(n).execute(&mut *tx).await?;
                sqlx::query("DELETE FROM stickers WHERE id=? AND type='markdown'")
                    .bind(n)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
}
fn preview(text: &str) -> String {
    text.chars()
        .take(120)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
/// Lowercase-hex SHA-256 of the exact bytes used to identify plaintext
/// duplicates. This is a content fingerprint, not a secret; see the
/// migration comment for the privacy contract.
pub(crate) fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
/// Persist fingerprints only where plaintext is already available. Never hash ciphertext.
pub(crate) async fn backfill_plaintext_digests(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<()> {
    let rows = sqlx::query("SELECT id,content FROM stickers WHERE type='markdown' AND text_digest='' AND content NOT GLOB 'RUSTICKERS_TEXT_*'")
        .fetch_all(&mut **tx).await?;
    for row in rows {
        let content: String = row.try_get("content")?;
        sqlx::query("UPDATE stickers SET text_digest=? WHERE id=?")
            .bind(digest(&content))
            .bind(row.try_get::<i64, _>("id")?)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn matching_sticker(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    digest: &str,
) -> Result<Option<i64>> {
    sqlx::query_scalar(
        "SELECT id FROM stickers WHERE type='markdown' AND text_digest=? ORDER BY id ASC LIMIT 1",
    )
    .bind(digest)
    .fetch_optional(&mut **tx)
    .await
    .map_err(Into::into)
}

/// Upgrade legacy plaintext identities and consume cross-table duplicates at startup.
pub(crate) async fn reconcile_captures(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<()> {
    backfill_plaintext_digests(tx).await?;
    let captures: Vec<(String, String)> =
        sqlx::query_as("SELECT text,text_digest FROM clipboard_captures")
            .fetch_all(&mut **tx)
            .await?;
    for (text, digest) in captures {
        if let Some(id) = matching_sticker(tx, &digest).await? {
            absorb_captures(tx, id, &text).await?;
        }
    }
    Ok(())
}

/// Consume disposable duplicates and preserve their activity in the saved identity.
/// Match exact plaintext here so even a legacy capture with a stale digest is removed.
pub(crate) async fn absorb_captures(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: i64,
    text: &str,
) -> Result<()> {
    sqlx::query("UPDATE stickers SET text_last_used_at=MAX(COALESCE(text_last_used_at,0),COALESCE((SELECT MAX(MAX(created_at,last_copied_at,COALESCE(last_used_at,0))) FROM clipboard_captures WHERE text=?),0)) WHERE id=?")
        .bind(text).bind(id).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM clipboard_captures WHERE text=?")
        .bind(text)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn key_in(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<Option<MasterKey>> {
    let value: Option<String> =
        sqlx::query_scalar("SELECT configuration FROM text_master_key WHERE singleton=1")
            .fetch_optional(&mut **tx)
            .await?;
    value.as_deref().map(MasterKey::parse).transpose()
}
async fn settings_in(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<ClipboardSettings> {
    let mut settings = ClipboardSettings::default();
    for row in sqlx::query("SELECT key,value FROM clipboard_settings")
        .fetch_all(&mut **tx)
        .await?
    {
        let key: String = row.try_get("key")?;
        let value: String = row.try_get("value")?;
        match key.as_str() {
            "normal_count_limit" => settings.normal_count_limit = value.parse()?,
            "normal_byte_limit" => settings.normal_byte_limit = value.parse()?,
            "capture_enabled" => settings.capture_enabled = value == "1",
            _ => {}
        }
    }
    Ok(settings)
}
async fn prune(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    settings: &ClipboardSettings,
) -> Result<()> {
    let rows = sqlx::query("SELECT id,byte_size FROM clipboard_captures ORDER BY MAX(created_at,last_copied_at,COALESCE(last_used_at,0)) DESC,id DESC")
        .fetch_all(&mut **tx).await?;
    let mut count = 0i64;
    let mut bytes = 0i64;
    let mut exhausted = false;
    for row in rows {
        let size: i64 = row.try_get("byte_size")?;
        count += 1;
        bytes = bytes.saturating_add(size);
        exhausted |= count > settings.normal_count_limit || bytes > settings.normal_byte_limit;
        if exhausted {
            sqlx::query("DELETE FROM clipboard_captures WHERE id=?")
                .bind(row.try_get::<i64, _>("id")?)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}
