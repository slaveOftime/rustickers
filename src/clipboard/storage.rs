//! SQLite-backed implementation of the clipboard history storage.
//!
//! The trait lives here so it is not shared with the sticker store. Callers should treat
//! [`SqliteClipboardStore`] as the only implementation in v1.

use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};

use super::items::{ClipboardItem, ClipboardItemKind, RetentionState};
use super::settings::ClipboardSettings;

/// Storage interface for the clipboard history.
///
/// Methods are split so the GUI chooser and CLI chooser never accidentally use a sticker-shaped
/// API. Snapshots are returned by value; backends must hand out a deep copy rather than a live
/// view that can be mutated by writers.
#[async_trait]
pub trait ClipboardHistoryStore: Send + Sync {
    async fn settings(&self) -> Result<ClipboardSettings>;
    async fn update_settings(&self, settings: &ClipboardSettings) -> Result<()>;

    /// Return every row the chooser could show, sorted MRU-first. The query, if present, is
    /// applied to the search columns; the chooser API still re-filters so it can update without
    /// a roundtrip.
    async fn snapshot(&self) -> Result<Vec<ClipboardItem>>;

    /// Cheap privacy-policy probe: does not load clipboard payloads or previews.
    async fn has_protected_items(&self) -> Result<bool>;

    /// Read one row by id.
    async fn get(&self, id: i64) -> Result<ClipboardItem>;

    /// Insert or update a row by content identity. The implementation decides whether the content
    /// is already present (plain-text digest for ordinary rows, the supplied match token for
    /// protected rows). Returns the row id and whether the row was newly created.
    async fn upsert_ordinary(
        &self,
        content: &str,
        plain_preview: &str,
        plaintext_digest: &str,
        byte_size: i64,
        now_ms: i64,
    ) -> Result<UpsertOutcome>;

    /// Store ciphertext only. Plaintext previews are rejected. An optional caller-authenticated
    /// keyed token deduplicates exact protected content; no token means a distinct row.
    async fn upsert_protected(
        &self,
        public_label: Option<&str>,
        plain_preview: Option<&str>,
        match_token: Option<&str>,
        envelope: &str,
        byte_size: i64,
        now_ms: i64,
    ) -> Result<UpsertOutcome>;

    /// Generation-bound protected insert. The expected vault envelope must be captured from
    /// the same unlocked key that sealed `envelope`, not freshly read from storage afterward.
    #[allow(clippy::too_many_arguments)]
    async fn upsert_protected_bound(
        &self,
        public_label: Option<&str>,
        plain_preview: Option<&str>,
        match_token: Option<&str>,
        envelope: &str,
        byte_size: i64,
        now_ms: i64,
        expected_vault_envelope: &str,
    ) -> Result<UpsertOutcome>;

    /// Protect an existing ordinary row without changing its identity, recency, or independent
    /// retention reasons. Rejects stale content, deletion, and concurrent vault reset/rewrap.
    /// Clears live plaintext columns only; this does not guarantee historical disk erasure.
    async fn compare_exchange_protection(
        &self,
        id: i64,
        expected_plaintext: &str,
        envelope: &str,
        expected_vault_envelope: &str,
        public_label: Option<&str>,
    ) -> Result<()>;
    /// Restore authenticated plaintext against the exact ciphertext and vault generation.
    async fn compare_exchange_unprotection(
        &self,
        id: i64,
        expected: &str,
        plaintext: &str,
        expected_vault_envelope: &str,
    ) -> Result<()>;
    /// Apply the eviction list returned by `clipboard::retention::eviction_victims`. The store
    /// runs the deletion in a transaction so an in-flight capture cannot half-evict.
    async fn delete_ordinary_by_ids(&self, ids: &[i64]) -> Result<()>;

    async fn set_retention_flag(&self, id: i64, flag: RetentionFlag, value: bool) -> Result<()>;

    async fn replace_content(
        &self,
        id: i64,
        content: &str,
        byte_size: i64,
        now_ms: i64,
    ) -> Result<()>;

    #[allow(clippy::too_many_arguments)]
    /// Save against the exact content read by the editor; fails on delete or concurrent edit.
    ///
    /// `label` is the public alias the row should carry after the save. Pass `Some(s)`
    /// to set `public_label = trim(s)` (or `NULL` when the trimmed string is empty);
    /// pass `None` to leave the column untouched. Empty-string and whitespace-only
    /// values always normalise to `NULL`, mirroring the protect path.
    async fn compare_exchange_content(
        &self,
        id: i64,
        expected: &str,
        replacement: &str,
        byte_size: i64,
        now_ms: i64,
        label: Option<&str>,
    ) -> Result<()>;

    /// Protected edits must supply fresh ciphertext and the ciphertext read on opening.
    async fn compare_exchange_protected_content(
        &self,
        id: i64,
        expected: &str,
        replacement: &str,
        byte_size: i64,
        now_ms: i64,
    ) -> Result<()>;

    #[allow(clippy::too_many_arguments)]
    /// Generation-bound counterpart of protected editing. Seal the replacement and capture
    /// its vault envelope together; ciphertext from a stale cached vault must not be stored.
    ///
    /// `label` follows the same convention as `compare_exchange_content`: `Some(s)` sets
    /// the trimmed value (or `NULL` when empty), `None` leaves the column untouched.
    async fn compare_exchange_protected_content_bound(
        &self,
        id: i64,
        expected: &str,
        replacement: &str,
        byte_size: i64,
        now_ms: i64,
        expected_vault_envelope: &str,
        label: Option<&str>,
    ) -> Result<()>;
    async fn mark_used(&self, id: i64, now_ms: i64) -> Result<()>;

    async fn delete_row(&self, id: i64) -> Result<()>;

    async fn read_vault_envelope(&self) -> Result<Option<String>>;
    async fn write_vault_envelope(&self, envelope: &str, active_until_unix_ms: i64) -> Result<()>;
    async fn clear_vault_envelope(&self) -> Result<()>;
    /// Compare-and-swap protects password changes against stale controllers.
    async fn compare_exchange_vault_envelope(
        &self,
        expected: &str,
        replacement: &str,
        active_until_unix_ms: i64,
    ) -> Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionFlag {
    Explicit,
    Edited,
    Protected,
}

impl RetentionFlag {
    fn column(self) -> &'static str {
        match self {
            RetentionFlag::Explicit => "retention_explicit",
            RetentionFlag::Edited => "retention_edited",
            RetentionFlag::Protected => "retention_protected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpsertOutcome {
    Inserted { id: i64 },
    Updated { id: i64 },
}

#[derive(Debug, Clone)]
pub struct SqliteClipboardStore {
    pool: SqlitePool,
}

impl SqliteClipboardStore {
    pub async fn open(db_path: impl AsRef<std::path::Path>) -> Result<Self> {
        let path = db_path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("create clipboard db parent dir")?;
        }
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .busy_timeout(std::time::Duration::from_secs(5))
            .pragma("secure_delete", "ON");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .context("connect clipboard sqlite pool")?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    /// Exact UTF-8 SHA-256 digest used as the ordinary-row dedupe key. It is not a secret.
    pub fn ordinary_digest(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let digest = hasher.finalize();
        let mut out = String::with_capacity(digest.len() * 2);
        for byte in digest {
            use std::fmt::Write;
            let _ = write!(&mut out, "{byte:02x}");
        }
        out
    }

    fn row_to_item(row: ClipboardRow) -> ClipboardItem {
        let kind = if row.protected_envelope.is_some() {
            ClipboardItemKind::Protected
        } else {
            ClipboardItemKind::Plain
        };
        ClipboardItem {
            id: row.id,
            kind,
            content: row.protected_envelope.unwrap_or(row.content),
            plain_preview: if kind == ClipboardItemKind::Protected {
                None
            } else {
                row.plain_preview
            },
            public_label: row.public_label,
            byte_size: row.byte_size,
            created_at: row.created_at,
            last_copied_at: row.last_copied_at,
            last_used_at: row.last_used_at,
            last_edited_at: row.last_edited_at,
            retention: RetentionState {
                explicit: row.retention_explicit != 0,
                edited: row.retention_edited != 0,
                protected_: kind == ClipboardItemKind::Protected || row.retention_protected != 0,
            },
        }
    }

    async fn require_vault_generation(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        expected: &str,
    ) -> Result<()> {
        ensure!(
            !expected.is_empty(),
            "protected writes require an expected vault envelope"
        );
        let current: Option<String> =
            sqlx::query_scalar("SELECT envelope FROM clipboard_vault WHERE id = 1")
                .fetch_optional(&mut **tx)
                .await?;
        ensure!(
            current.as_deref() == Some(expected),
            "vault changed or was disabled; unlock again before saving"
        );
        Ok(())
    }
    /// Called under an immediate write transaction: capture and limits cannot race pruning.
    async fn trim_ordinary(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<()> {
        let settings: Vec<(String, String)> =
            sqlx::query_as("SELECT key, value FROM clipboard_settings")
                .fetch_all(&mut **tx)
                .await?;
        let mut limits = ClipboardSettings::default();
        for (key, value) in settings {
            match key.as_str() {
                "normal_count_limit" => limits.normal_count_limit = value.parse()?,
                "normal_byte_limit" => limits.normal_byte_limit = value.parse()?,
                _ => {}
            }
        }
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT id, byte_size FROM clipboard_items WHERE protected_envelope IS NULL
             AND retention_explicit = 0 AND retention_edited = 0 AND retention_protected = 0
             ORDER BY MAX(created_at, last_copied_at, COALESCE(last_used_at, 0),
                 COALESCE(last_edited_at, 0)), created_at, id",
        )
        .fetch_all(&mut **tx)
        .await?;
        let mut count = rows.len() as i64;
        let mut bytes: i64 = rows.iter().map(|(_, size)| size).sum();
        for (id, size) in rows {
            if (limits.normal_count_limit <= 0 || count <= limits.normal_count_limit)
                && (limits.normal_byte_limit <= 0 || bytes <= limits.normal_byte_limit)
            {
                break;
            }
            sqlx::query("DELETE FROM clipboard_items WHERE id = ?1")
                .bind(id)
                .execute(&mut **tx)
                .await?;
            count -= 1;
            bytes -= size;
        }
        Ok(())
    }
    async fn read_settings(&self) -> Result<ClipboardSettings> {
        let row =
            sqlx::query_as::<_, (String, String)>("SELECT key, value FROM clipboard_settings")
                .fetch_all(&self.pool)
                .await
                .context("read clipboard settings")?;
        let mut s = ClipboardSettings::default();
        for (k, v) in row {
            match k.as_str() {
                "normal_count_limit" => {
                    if let Ok(n) = v.parse() {
                        s.normal_count_limit = n;
                    }
                }
                "normal_byte_limit" => {
                    if let Ok(n) = v.parse() {
                        s.normal_byte_limit = n;
                    }
                }
                "capture_enabled" => {
                    s.capture_enabled = matches!(v.as_str(), "1" | "true");
                }
                _ => {}
            }
        }
        Ok(s)
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ClipboardRow {
    id: i64,
    content: String,
    plain_preview: Option<String>,
    public_label: Option<String>,
    retention_explicit: i64,
    retention_edited: i64,
    retention_protected: i64,
    protected_envelope: Option<String>,
    created_at: i64,
    last_copied_at: i64,
    last_used_at: Option<i64>,
    last_edited_at: Option<i64>,
    byte_size: i64,
}

#[async_trait]
impl ClipboardHistoryStore for SqliteClipboardStore {
    async fn settings(&self) -> Result<ClipboardSettings> {
        self.read_settings().await
    }

    async fn update_settings(&self, settings: &ClipboardSettings) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let pairs: [(&str, String); 3] = [
            (
                "normal_count_limit",
                settings.normal_count_limit.to_string(),
            ),
            ("normal_byte_limit", settings.normal_byte_limit.to_string()),
            (
                "capture_enabled",
                if settings.capture_enabled {
                    "1".into()
                } else {
                    "0".into()
                },
            ),
        ];
        for (k, v) in pairs {
            sqlx::query(
                "INSERT INTO clipboard_settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )
            .bind(k)
            .bind(v)
            .execute(&mut *tx)
            .await
            .context("update clipboard setting")?;
        }
        Self::trim_ordinary(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn has_protected_items(&self) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM clipboard_items WHERE protected_envelope IS NOT NULL)",
        )
        .fetch_one(&self.pool)
        .await?)
    }
    async fn snapshot(&self) -> Result<Vec<ClipboardItem>> {
        let rows = sqlx::query_as::<_, ClipboardRow>(
            r#"SELECT id, content, plain_preview, public_label,
                      retention_explicit, retention_edited, retention_protected,
                      protected_envelope, created_at, last_copied_at,
                      last_used_at, last_edited_at, byte_size
               FROM clipboard_items
               ORDER BY id DESC"#,
        )
        .fetch_all(&self.pool)
        .await
        .context("snapshot clipboard items")?;
        let mut items: Vec<ClipboardItem> = rows.into_iter().map(Self::row_to_item).collect();
        super::ordering::sort_in_place(&mut items);
        Ok(items)
    }

    async fn get(&self, id: i64) -> Result<ClipboardItem> {
        let row = sqlx::query_as::<_, ClipboardRow>(
            r#"SELECT id, content, plain_preview, public_label,
                      retention_explicit, retention_edited, retention_protected,
                      protected_envelope, created_at, last_copied_at,
                      last_used_at, last_edited_at, byte_size
               FROM clipboard_items
               WHERE id = ?1"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .context("get clipboard row")?
        .ok_or_else(|| anyhow::anyhow!("clipboard row {id} not found"))?;
        Ok(Self::row_to_item(row))
    }

    async fn upsert_ordinary(
        &self,
        content: &str,
        plain_preview: &str,
        plaintext_digest: &str,
        byte_size: i64,
        now_ms: i64,
    ) -> Result<UpsertOutcome> {
        ensure!(
            plaintext_digest == Self::ordinary_digest(content),
            "clipboard digest must match exact content"
        );
        ensure!(
            byte_size == content.len() as i64,
            "clipboard byte size must match content"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let byte_limit: Option<String> = sqlx::query_scalar(
            "SELECT value FROM clipboard_settings WHERE key = 'normal_byte_limit'",
        )
        .fetch_optional(&mut *tx)
        .await?;
        let byte_limit = byte_limit
            .map(|value| value.parse::<i64>())
            .transpose()?
            .unwrap_or(ClipboardSettings::default().normal_byte_limit);
        ensure!(
            byte_limit <= 0 || byte_size <= byte_limit,
            "clipboard content exceeds the ordinary byte limit"
        );
        // No authorized keyed match check is supplied here: fail closed on recapture.
        let protected: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM clipboard_items WHERE protected_envelope IS NOT NULL)",
        )
        .fetch_one(&mut *tx)
        .await?;
        ensure!(
            !protected,
            "ordinary collection paused: protected history requires an authorized match check"
        );
        let existing: Option<(i64, i64)> = sqlx::query_as(
            "SELECT id, byte_size FROM clipboard_items
             WHERE plaintext_digest = ?1 AND protected_envelope IS NULL AND content = ?2
             LIMIT 1",
        )
        .bind(plaintext_digest)
        .bind(content)
        .fetch_optional(&mut *tx)
        .await
        .context("lookup digest")?;

        if let Some((id, _)) = existing {
            sqlx::query(
                "UPDATE clipboard_items
                 SET last_copied_at = MAX(last_copied_at, ?1), byte_size = ?2, content_bytes = ?2, content = ?3, plain_preview = ?4
                 WHERE id = ?5",
            )
            .bind(now_ms)
            .bind(byte_size)
            .bind(content)
            .bind(plain_preview)
            .bind(id)
            .execute(&mut *tx)
            .await
            .context("update ordinary row")?;
            Self::trim_ordinary(&mut tx).await?;
            tx.commit().await?;
            return Ok(UpsertOutcome::Updated { id });
        }

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO clipboard_items (
                content, content_bytes, plain_preview, plaintext_digest,
                retention_explicit, retention_edited, retention_protected, locked,
                created_at, last_copied_at, byte_size
             ) VALUES (?1, ?2, ?3, ?4, 0, 0, 0, 0, ?5, ?6, ?7)
             RETURNING id",
        )
        .bind(content)
        .bind(byte_size)
        .bind(plain_preview)
        .bind(plaintext_digest)
        .bind(now_ms)
        .bind(now_ms)
        .bind(byte_size)
        .fetch_one(&mut *tx)
        .await
        .context("insert ordinary row")?;
        Self::trim_ordinary(&mut tx).await?;
        tx.commit().await?;
        Ok(UpsertOutcome::Inserted { id })
    }

    async fn upsert_protected(
        &self,
        _public_label: Option<&str>,
        _plain_preview: Option<&str>,
        _match_token: Option<&str>,
        _envelope: &str,
        _byte_size: i64,
        _now_ms: i64,
    ) -> Result<UpsertOutcome> {
        anyhow::bail!("protected writes require a vault generation; use upsert_protected_bound")
    }
    #[allow(clippy::too_many_arguments)]
    async fn upsert_protected_bound(
        &self,
        public_label: Option<&str>,
        plain_preview: Option<&str>,
        match_token: Option<&str>,
        envelope: &str,
        byte_size: i64,
        now_ms: i64,
        expected_vault_envelope: &str,
    ) -> Result<UpsertOutcome> {
        ensure!(
            plain_preview.is_none(),
            "protected rows cannot persist plaintext previews"
        );
        ensure!(byte_size >= 0, "clipboard byte size cannot be negative");
        super::vault::validate_sealed(envelope)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        Self::require_vault_generation(&mut tx, expected_vault_envelope).await?;
        if let Some(token) = match_token {
            let existing: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM clipboard_items WHERE protected_envelope IS NOT NULL AND protected_match_token = ?1 LIMIT 1")
                .bind(token).fetch_optional(&mut *tx).await?;
            if let Some(id) = existing {
                sqlx::query("UPDATE clipboard_items SET last_copied_at = MAX(last_copied_at, ?1) WHERE id = ?2")
                    .bind(now_ms).bind(id).execute(&mut *tx).await?;
                tx.commit().await?;
                return Ok(UpsertOutcome::Updated { id });
            }
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO clipboard_items (
                content, content_bytes, plain_preview, protected_match_token,
                protected_envelope, public_label,
                retention_explicit, retention_edited, retention_protected, locked,
                created_at, last_copied_at, byte_size
             ) VALUES ('', ?1, ?2, ?3, ?4, ?5, 0, 0, 1, 1, ?6, ?7, ?1)
             RETURNING id",
        )
        .bind(byte_size)
        .bind(plain_preview)
        .bind(match_token)
        .bind(envelope)
        .bind(public_label)
        .bind(now_ms)
        .bind(now_ms)
        .fetch_one(&mut *tx)
        .await
        .context("insert protected row")?;
        tx.commit().await?;
        Ok(UpsertOutcome::Inserted { id })
    }

    async fn compare_exchange_protection(
        &self,
        id: i64,
        expected_plaintext: &str,
        envelope: &str,
        expected_vault_envelope: &str,
        public_label: Option<&str>,
    ) -> Result<()> {
        if let Some(label) = public_label {
            ensure!(!label.trim().is_empty(), "A public label is required");
        }
        super::vault::validate_sealed(envelope)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        Self::require_vault_generation(&mut tx, expected_vault_envelope).await?;
        // Read and compare while holding SQLite's writer lock; do not trust a chooser snapshot.
        let current: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT content, protected_envelope FROM clipboard_items WHERE id = ?1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let (plaintext, protected) = current.context("clipboard row no longer exists")?;
        let plaintext = zeroize::Zeroizing::new(plaintext);
        ensure!(
            protected.is_none() && plaintext.as_str() == expected_plaintext,
            "clipboard row changed; reload before protecting"
        );
        sqlx::query(
            "UPDATE clipboard_items SET protected_envelope = ?1, content = '',
            plain_preview = NULL, plaintext_digest = NULL, protected_match_token = NULL,
            content_bytes = ?2, byte_size = ?2, retention_protected = 1, locked = 1, public_label = ?4
            WHERE id = ?3",
        )
        .bind(envelope)
        .bind(expected_plaintext.len() as i64)
        .bind(id)
        .bind(public_label.map(str::trim))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    async fn compare_exchange_unprotection(
        &self,
        id: i64,
        expected: &str,
        plaintext: &str,
        expected_vault_envelope: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        Self::require_vault_generation(&mut tx, expected_vault_envelope).await?;
        let affected = sqlx::query("UPDATE clipboard_items SET content = ?1, plain_preview = substr(?1, 1, 240), plaintext_digest = ?2,
            protected_envelope = NULL, protected_match_token = NULL, public_label = NULL,
            retention_protected = 0, locked = 0, content_bytes = ?3, byte_size = ?3
            WHERE id = ?4 AND protected_envelope = ?5")
            .bind(plaintext).bind(Self::ordinary_digest(plaintext)).bind(plaintext.len() as i64)
            .bind(id).bind(expected).execute(&mut *tx).await?.rows_affected();
        ensure!(
            affected == 1,
            "protected row changed or was deleted; reload before unlocking"
        );
        tx.commit().await?;
        Ok(())
    }
    async fn delete_ordinary_by_ids(&self, id: &[i64]) -> Result<()> {
        if id.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let placeholders = std::iter::repeat_n("?", id.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "DELETE FROM clipboard_items
             WHERE id IN ({placeholders})
               AND retention_explicit = 0
               AND retention_edited = 0
               AND retention_protected = 0
               AND protected_envelope IS NULL"
        );
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
        for id in id {
            query = query.bind(id);
        }
        query
            .execute(&mut *tx)
            .await
            .context("delete ordinary rows")?;
        tx.commit().await?;
        Ok(())
    }

    async fn set_retention_flag(&self, id: i64, flag: RetentionFlag, value: bool) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row: Option<(Option<String>, i64)> = sqlx::query_as(
            "SELECT protected_envelope, retention_protected FROM clipboard_items WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let (envelope, reason) = row.context("clipboard row no longer exists")?;
        if flag == RetentionFlag::Protected {
            ensure!(
                !value || envelope.is_some() || reason != 0,
                "protect requires encrypted content, not a retention flag"
            );
            ensure!(
                value || envelope.is_none(),
                "unprotect requires authenticated decryption"
            );
        }
        let column = flag.column();
        let sql = format!("UPDATE clipboard_items SET {column} = ?1 WHERE id = ?2");
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(i64::from(value))
            .bind(id)
            .execute(&mut *tx)
            .await
            .context("set retention flag")?;
        tx.commit().await?;
        Ok(())
    }

    async fn replace_content(
        &self,
        id: i64,
        content: &str,
        byte_size: i64,
        now_ms: i64,
    ) -> Result<()> {
        ensure!(
            byte_size == content.len() as i64,
            "clipboard byte size must match content"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT content, protected_envelope FROM clipboard_items WHERE id = ?1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let (previous, envelope) = row.context("clipboard row no longer exists")?;
        ensure!(
            envelope.is_none(),
            "protected editing requires re-encryption"
        );
        if previous != content {
            sqlx::query(
                "UPDATE clipboard_items SET content = ?1, byte_size = ?2,
                content_bytes = ?2, last_edited_at = ?3, retention_edited = 1,
                plain_preview = substr(?1, 1, 240), plaintext_digest = ?4 WHERE id = ?5",
            )
            .bind(content)
            .bind(byte_size)
            .bind(now_ms)
            .bind(Self::ordinary_digest(content))
            .bind(id)
            .execute(&mut *tx)
            .await
            .context("replace content")?;
        }
        tx.commit().await?;
        Ok(())
    }
    async fn compare_exchange_content(
        &self,
        id: i64,
        expected: &str,
        replacement: &str,
        byte_size: i64,
        now_ms: i64,
        label: Option<&str>,
    ) -> Result<()> {
        ensure!(
            byte_size == replacement.len() as i64,
            "clipboard byte size must match content"
        );
        // Some("") / Some("   ") reduce to None; None leaves the column untouched.
        let normalised_label: Option<&str> = label.map(str::trim).filter(|s| !s.is_empty());
        let affected = sqlx::query(
            "UPDATE clipboard_items SET content = ?1,
            byte_size = ?2, content_bytes = ?2,
            last_edited_at = CASE WHEN content != ?1 THEN ?3 ELSE last_edited_at END,
            retention_edited = CASE WHEN content != ?1 THEN 1 ELSE retention_edited END,
            plain_preview = substr(?1, 1, 240), plaintext_digest = ?4
            WHERE id = ?5 AND content = ?6 AND protected_envelope IS NULL",
        )
        .bind(replacement)
        .bind(byte_size)
        .bind(now_ms)
        .bind(Self::ordinary_digest(replacement))
        .bind(id)
        .bind(expected)
        .execute(&self.pool)
        .await?
        .rows_affected();
        ensure!(
            affected == 1,
            "clipboard row changed or was deleted; reload before saving"
        );
        // Apply the label update for `Some(...)`: the editor read the row at
        // open time and is now writing back, so the user is being authoritative.
        // Bind `NULL` when the caller passed whitespace/empty so `Some("")`
        // becomes a "clear", not a no-op. `None` is a separate signal that
        // means "column untouched" -- we exercise it when the labelled save
        // races with content-only saves.
        if label.is_some() {
            sqlx::query("UPDATE clipboard_items SET public_label = ?1 WHERE id = ?2")
                .bind(normalised_label)
                .bind(id)
                .execute(&self.pool)
                .await
                .context("update public_label")?;
        }
        Ok(())
    }

    async fn compare_exchange_protected_content(
        &self,
        _id: i64,
        _expected: &str,
        _replacement: &str,
        _byte_size: i64,
        _now_ms: i64,
    ) -> Result<()> {
        anyhow::bail!(
            "protected edits require a vault generation; use compare_exchange_protected_content_bound"
        )
    }
    async fn compare_exchange_protected_content_bound(
        &self,
        id: i64,
        expected: &str,
        replacement: &str,
        byte_size: i64,
        now_ms: i64,
        expected_vault_envelope: &str,
        label: Option<&str>,
    ) -> Result<()> {
        super::vault::validate_sealed(replacement)?;
        ensure!(byte_size >= 0, "clipboard byte size cannot be negative");
        // Some("") / Some("   ") reduce to None; None leaves the column untouched.
        let normalised_label: Option<&str> = label.map(str::trim).filter(|s| !s.is_empty());
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        Self::require_vault_generation(&mut tx, expected_vault_envelope).await?;
        let affected = sqlx::query("UPDATE clipboard_items SET protected_envelope = ?1,
            content = '', plain_preview = NULL, plaintext_digest = NULL, protected_match_token = NULL,
            byte_size = ?2, content_bytes = ?2, last_edited_at = ?3,
            retention_edited = 1, retention_protected = 1
            WHERE id = ?4 AND protected_envelope = ?5")
            .bind(replacement).bind(byte_size).bind(now_ms).bind(id).bind(expected)
            .execute(&mut *tx).await?.rows_affected();
        ensure!(
            affected == 1,
            "protected row changed or was deleted; reload before saving"
        );
        // The encrypt path already binds `public_label` into the same UPDATE; for
        // encrypted edits, the editor surfaces this label in the unprotect-protected
        // handoff. Some(...) writes the authoritative value (NULL passes through to
        // the column, so whitespace-only inputs become a clear). None skips so
        // the column is left alone.
        if label.is_some() {
            sqlx::query("UPDATE clipboard_items SET public_label = ?1 WHERE id = ?2")
                .bind(normalised_label)
                .bind(id)
                .execute(&mut *tx)
                .await
                .context("update public_label")?;
        }
        tx.commit().await?;
        Ok(())
    }
    async fn mark_used(&self, id: i64, now_ms: i64) -> Result<()> {
        sqlx::query("UPDATE clipboard_items SET last_used_at = ?1 WHERE id = ?2")
            .bind(now_ms)
            .bind(id)
            .execute(&self.pool)
            .await
            .context("mark used")?;
        Ok(())
    }

    async fn delete_row(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM clipboard_items WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("delete row")?;
        Ok(())
    }

    async fn read_vault_envelope(&self) -> Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT envelope FROM clipboard_vault WHERE id = 1")
                .fetch_optional(&self.pool)
                .await
                .context("read vault envelope")?;
        let Some((text,)) = row else {
            return Ok(None);
        };
        if text.is_empty() {
            Ok(None)
        } else {
            Ok(Some(text))
        }
    }

    async fn write_vault_envelope(&self, envelope: &str, active_until_unix_ms: i64) -> Result<()> {
        super::vault::VaultEnvelope::parse(envelope)?;
        let affected = sqlx::query(
            "UPDATE clipboard_vault SET envelope = ?1, active_until_unix_ms = ?2
            WHERE id = 1 AND envelope = ''",
        )
        .bind(envelope)
        .bind(active_until_unix_ms)
        .execute(&self.pool)
        .await?
        .rows_affected();
        ensure!(
            affected == 1,
            "vault already configured; authenticate and rewrap instead"
        );
        Ok(())
    }

    async fn clear_vault_envelope(&self) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let protected: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM clipboard_items WHERE protected_envelope IS NOT NULL)",
        )
        .fetch_one(&mut *tx)
        .await?;
        ensure!(
            !protected,
            "cannot disable a vault while protected history exists"
        );
        sqlx::query(
            "UPDATE clipboard_vault SET envelope = '', active_until_unix_ms = 0 WHERE id = 1",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn compare_exchange_vault_envelope(
        &self,
        expected: &str,
        replacement: &str,
        active_until_unix_ms: i64,
    ) -> Result<()> {
        ensure!(!expected.is_empty(), "rewrap requires an existing vault");
        super::vault::VaultEnvelope::parse(replacement)?;
        let affected = sqlx::query(
            "UPDATE clipboard_vault SET envelope = ?1, active_until_unix_ms = ?2
            WHERE id = 1 AND envelope = ?3",
        )
        .bind(replacement)
        .bind(active_until_unix_ms)
        .bind(expected)
        .execute(&self.pool)
        .await?
        .rows_affected();
        ensure!(affected == 1, "vault changed concurrently; unlock again");
        Ok(())
    }
}
