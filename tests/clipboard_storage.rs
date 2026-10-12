//! Capture and read-only legacy import contracts on isolated SQLite files.
#![cfg(feature = "storage")]

use anyhow::Context as _;
use futures::executor::block_on;
use rustickers::text::{CaptureOutcome, TextEntryId, TextRepository, TextSources};
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};

async fn pool(path: &std::path::Path) -> anyhow::Result<SqlitePool> {
    Ok(SqlitePool::connect_with(
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
    )
    .await?)
}

async fn legacy(path: &std::path::Path) -> anyhow::Result<SqlitePool> {
    let pool = pool(path).await?;
    sqlx::raw_sql(include_str!("../migrations/0010_add_clipboard_history.sql"))
        .execute(&pool)
        .await?;
    Ok(pool)
}

async fn raw(
    pool: &SqlitePool,
    text: &str,
    created: i64,
    copied: i64,
    used: Option<i64>,
) -> anyhow::Result<()> {
    // Deliberately inaccurate metadata and obsolete flags must not affect raw import.
    sqlx::query("INSERT INTO clipboard_items(content,content_bytes,plain_preview,plaintext_digest,protected_envelope,public_label,retention_explicit,retention_edited,retention_protected,locked,created_at,last_copied_at,last_used_at,last_edited_at,byte_size) VALUES(?,999,'wrong preview','wrong digest','RUSTICKERS_VAULT_V1\nold envelope','old label',1,1,1,1,?,?,?,9999,999)")
        .bind(text).bind(created).bind(copied).bind(used).execute(pool).await?;
    Ok(())
}

#[test]
fn capture_round_trip_persists_without_legacy_vault_dependency() -> anyhow::Result<()> {
    block_on(async {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("stickers.db");
        let repo = TextRepository::open(&path).await?;
        let settings = repo.settings().await?;
        assert!(
            !settings.capture_enabled,
            "observer capture defaults to paused"
        );
        assert_eq!(settings.normal_count_limit, 500);
        assert_eq!(settings.normal_byte_limit, 20 * 1024 * 1024);
        assert!(repo.master_key().await?.is_none());
        let exact = " \t大文字\r\n'quoted' ; & ";
        let id = repo
            .capture(exact, 10)
            .await?
            .capture_id()
            .context("expected a stored clipboard capture")?;
        assert_eq!(repo.capture(exact, 20).await?, CaptureOutcome::Stored(id));
        let reopened = TextRepository::open(&path).await?;
        let rows = reopened.snapshot(TextSources::new(false, true), "").await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id(), TextEntryId::Clipboard(id));
        assert_eq!(rows[0].activity_at(), 20);
        assert_eq!(&*reopened.resolve(&rows[0], None).await?, exact);
        assert!(
            !dir.path().join("clipboard.db").exists(),
            "opening the canonical store must not create an old backend"
        );
        Ok(())
    })
}

#[test]
fn legacy_raw_import_ignores_flags_recomputes_bytes_and_is_restart_safe() -> anyhow::Result<()> {
    block_on(async {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("stickers.db");
        let old_path = dir.path().join("clipboard.db");
        let old = legacy(&old_path).await?;
        let exact = " \tRaw 雪\r\n'keep' ";
        raw(&old, exact, 10, 20, Some(30)).await?;
        raw(&old, exact, 15, 40, Some(50)).await?;
        raw(&old, "raw 雪", 60, 60, None).await?;
        let invalid = [
            "",
            " \r\n\t",
            "bad\0text",
            "RUSTICKERS_VAULT_V1\nsecret",
            " RUSTICKERS_LOCKED_V1\nsecret",
            "RUSTICKERS_TEXT_V2\nsecret",
            "RUSTICKERS_TEXT_V1\ngeneration\nciphertext",
        ];
        for text in invalid {
            raw(&old, text, 1, 1, None).await?;
        }
        raw(&old, &"x".repeat(1024 * 1024 + 1), 1, 1, None).await?;
        old.close().await;
        let original_file = std::fs::read(&old_path)?;
        let repo = TextRepository::open(&path).await?; // Startup owns the one-shot import.
        let db = pool(&path).await?;
        let rows = repo.snapshot(TextSources::new(false, true), "").await?;
        assert_eq!(rows.len(), 2);
        let imported = rows
            .iter()
            .find(|row| row.matches("Raw") && row.activity_at() == 50)
            .unwrap();
        assert_eq!(&*repo.resolve(imported, None).await?, exact);
        assert!(!imported.matches("old label"));
        assert!(!imported.matches("wrong preview"));
        let metadata: (i64, i64, Option<i64>) = sqlx::query_as(
            "SELECT byte_size,last_copied_at,last_used_at FROM clipboard_captures WHERE id=?",
        )
        .bind(match imported.id() {
            TextEntryId::Clipboard(id) => id,
            _ => unreachable!(),
        })
        .fetch_one(&db)
        .await?;
        assert_eq!(metadata, (exact.len() as i64, 40, Some(50)));
        let counts: (i64, i64) = sqlx::query_as(
            "SELECT imported_count,skipped_count FROM text_imports WHERE source='legacy-clipboard'",
        )
        .fetch_one(&db)
        .await?;
        assert_eq!(counts, (2, 8));
        repo.delete(imported.id()).await?;
        let reopened = TextRepository::open(&path).await?;
        reopened.import_legacy(&old_path).await?;
        assert!(
            reopened.get(imported.id()).await.is_err(),
            "deleted imported text must not resurrect on restart"
        );
        assert_eq!(
            reopened
                .snapshot(TextSources::new(false, true), "")
                .await?
                .len(),
            1
        );
        assert_eq!(
            std::fs::read(&old_path)?,
            original_file,
            "legacy database must be byte-for-byte untouched"
        );
        Ok(())
    })
}

#[test]
fn legacy_settings_are_preserved_and_import_obeys_their_budgets() -> anyhow::Result<()> {
    block_on(async {
        let dir = tempfile::tempdir()?;
        let old = legacy(&dir.path().join("clipboard.db")).await?;
        sqlx::query("UPDATE clipboard_settings SET value=CASE key WHEN 'normal_count_limit' THEN '2' WHEN 'normal_byte_limit' THEN '4' WHEN 'capture_enabled' THEN '1' ELSE value END")
            .execute(&old).await?;
        for (text, time) in [("aa", 10), ("bb", 20), ("cc", 30)] {
            raw(&old, text, time, time, None).await?;
        }
        old.close().await;
        let repo = TextRepository::open(dir.path().join("stickers.db")).await?;
        let settings = repo.settings().await?;
        assert_eq!(
            settings.normal_count_limit, 2,
            "cutover must preserve the user's legacy count budget"
        );
        assert_eq!(
            settings.normal_byte_limit, 4,
            "cutover must preserve the user's legacy byte budget"
        );
        assert!(
            settings.capture_enabled,
            "cutover must preserve the explicit collection opt-in"
        );
        let rows = repo.snapshot(TextSources::new(false, true), "").await?;
        assert_eq!(rows.len(), 2);
        assert_eq!(&*repo.resolve(&rows[0], None).await?, "cc");
        assert_eq!(&*repo.resolve(&rows[1], None).await?, "bb");
        Ok(())
    })
}

#[test]
fn import_merges_canonical_raw_rows_with_external_rows_and_never_replays_them() -> anyhow::Result<()>
{
    block_on(async {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("stickers.db");
        let repo = TextRepository::open(&path).await?;
        let db = pool(&path).await?;
        raw(&db, "canonical raw", 1, 10, Some(20)).await?;
        let old_path = dir.path().join("clipboard.db");
        let old = legacy(&old_path).await?;
        raw(&old, "canonical raw", 2, 30, Some(40)).await?;
        raw(&old, "external raw", 3, 50, None).await?;
        old.close().await;
        repo.import_legacy(&old_path).await?;
        let rows = repo.snapshot(TextSources::new(false, true), "").await?;
        assert_eq!(rows.len(), 2);
        let canonical = rows.iter().find(|row| row.matches("canonical")).unwrap();
        assert_eq!(canonical.activity_at(), 40);
        assert_eq!(&*repo.resolve(canonical, None).await?, "canonical raw");
        let counts: (i64, i64) = sqlx::query_as(
            "SELECT imported_count,skipped_count FROM text_imports WHERE source='legacy-clipboard'",
        )
        .fetch_one(&db)
        .await?;
        assert_eq!(counts, (2, 0));
        repo.delete(canonical.id()).await?;
        repo.import_legacy(&old_path).await?;
        assert!(repo.get(canonical.id()).await.is_err());
        Ok(())
    })
}

#[test]
fn absent_legacy_file_does_not_mark_import_complete_or_prevent_later_import() -> anyhow::Result<()>
{
    block_on(async {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("stickers.db");
        let old_path = dir.path().join("clipboard.db");
        let repo = TextRepository::open(&path).await?;
        repo.import_legacy(&old_path).await?;
        let db = pool(&path).await?;
        let markers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM text_imports")
            .fetch_one(&db)
            .await?;
        assert_eq!(markers, 0);
        assert!(!old_path.exists());
        let old = legacy(&old_path).await?;
        raw(&old, "arrived later", 1, 2, None).await?;
        old.close().await;
        repo.import_legacy(&old_path).await?;
        assert_eq!(
            repo.snapshot(TextSources::new(false, true), "")
                .await?
                .len(),
            1
        );
        Ok(())
    })
}

#[test]
fn failed_import_rolls_back_rows_and_marker_then_can_retry() -> anyhow::Result<()> {
    block_on(async {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("stickers.db");
        let repo = TextRepository::open(&path).await?;
        let db = pool(&path).await?;
        let old_path = dir.path().join("clipboard.db");
        let old = legacy(&old_path).await?;
        raw(&old, "first", 1, 1, None).await?;
        raw(&old, "reject", 2, 2, None).await?;
        old.close().await;
        sqlx::raw_sql("CREATE TRIGGER fail_import BEFORE INSERT ON clipboard_captures WHEN NEW.text='reject' BEGIN SELECT RAISE(ABORT,'injected import failure'); END;")
            .execute(&db).await?;
        assert!(repo.import_legacy(&old_path).await.is_err());
        assert!(
            repo.snapshot(TextSources::new(false, true), "")
                .await?
                .is_empty()
        );
        let markers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM text_imports")
            .fetch_one(&db)
            .await?;
        assert_eq!(markers, 0, "failed import must remain retryable");
        sqlx::query("DROP TRIGGER fail_import").execute(&db).await?;
        repo.import_legacy(&old_path).await?;
        assert_eq!(
            repo.snapshot(TextSources::new(false, true), "")
                .await?
                .len(),
            2
        );
        Ok(())
    })
}
