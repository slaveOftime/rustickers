#![cfg(feature = "storage")]

use futures::executor::block_on;
use rustickers::{
    model::sticker::StickerState,
    text::{TextEntryId, TextRepository, TextSources, filter_indices},
};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tempfile::TempDir;

async fn fixture() -> anyhow::Result<(TempDir, TextRepository, SqlitePool)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("stickers.db");
    let repo = TextRepository::open(&path).await?;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(&path))
        .await?;
    Ok((dir, repo, pool))
}

async fn sticker(pool: &SqlitePool, title: &str, text: &str, time: i64) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar("INSERT INTO stickers(title,state,left,top,width,height,color,type,content,created_at,updated_at,text_modified_at) VALUES(?,'close',100,100,400,300,'yellow','markdown',?,?,?,?) RETURNING id")
        .bind(title).bind(text).bind(time).bind(time).bind(time).fetch_one(pool).await?)
}

#[test]
fn multiline_preview_is_bounded_without_splitting_unicode() -> anyhow::Result<()> {
    block_on(async {
        let (_dir, repo, _) = fixture().await?;
        let text = "界\n".repeat(600);
        let id = repo.capture(&text, 1).await?;
        let entry = repo.get(TextEntryId::Clipboard(id)).await?;
        let preview = entry.multiline_preview();
        assert_eq!(preview.chars().count(), 1024);
        assert!(text.starts_with(&preview));
        assert_eq!(repo.resolve(&entry, None).await?.as_str(), text);
        Ok(())
    })
}

#[test]
fn exact_capture_dedupe_and_validation() -> anyhow::Result<()> {
    block_on(async {
        let (_dir, repo, pool) = fixture().await?;
        let text = " \tRust 大文字\r\n'quoted' \"x\"; !\n ";
        let id = repo.capture(text, 10).await?;
        assert_eq!(repo.capture(text, 20).await?, id);
        assert_eq!(repo.capture(text, 15).await?, id);
        let entry = repo.get(TextEntryId::Clipboard(id)).await?;
        assert_eq!(&*repo.resolve(&entry, None).await?, text);
        assert_eq!(entry.multiline_preview(), text);
        assert_eq!(entry.activity_at(), 20);
        let row: (i64, i64, i64) = sqlx::query_as(
            "SELECT created_at,last_copied_at,byte_size FROM clipboard_captures WHERE id=?",
        )
        .bind(id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(row, (10, 20, text.len() as i64));
        assert_ne!(repo.capture(&text.to_lowercase(), 21).await?, id);
        // Even a colliding digest is only an accelerator, never an identity.
        sqlx::query("UPDATE clipboard_captures SET text='different bytes' WHERE id=?")
            .bind(id)
            .execute(&pool)
            .await?;
        assert_ne!(repo.capture(text, 22).await?, id);
        for invalid in ["", " \r\n\t", "a\0b"] {
            assert!(repo.capture(invalid, 30).await.is_err());
        }
        assert!(
            repo.capture(&"x".repeat(1024 * 1024 + 1), 30)
                .await
                .is_err()
        );
        Ok(())
    })
}

#[test]
fn lowered_count_and_byte_limits_prune_only_captures() -> anyhow::Result<()> {
    block_on(async {
        let (_dir, repo, pool) = fixture().await?;
        let saved = sticker(&pool, "saved", "never evict", 1).await?;
        let a = repo.capture("aa", 10).await?;
        let b = repo.capture("éé", 20).await?;
        let c = repo.capture("cc", 30).await?;
        let mut settings = repo.settings().await?;
        settings.normal_count_limit = 2;
        repo.update_settings(&settings).await?;
        assert!(repo.get(TextEntryId::Clipboard(a)).await.is_err());
        assert_eq!(
            repo.snapshot(TextSources::new(false, true), "")
                .await?
                .len(),
            2
        );
        settings.normal_byte_limit = 2;
        repo.update_settings(&settings).await?;
        assert!(repo.get(TextEntryId::Clipboard(b)).await.is_err());
        assert!(repo.get(TextEntryId::Clipboard(c)).await.is_ok());
        assert!(repo.get(TextEntryId::Sticker(saved)).await.is_ok());
        settings.normal_count_limit = -1;
        assert!(repo.update_settings(&settings).await.is_err());
        assert_eq!(repo.settings().await?.normal_count_limit, 2);
        settings.normal_count_limit = 0;
        repo.update_settings(&settings).await?;
        assert!(
            repo.snapshot(TextSources::new(false, true), "")
                .await?
                .is_empty()
        );
        assert!(repo.get(TextEntryId::Sticker(saved)).await.is_ok());
        Ok(())
    })
}

#[test]
fn promotion_consumes_source_once_and_stale_or_failed_promotion_rolls_back() -> anyhow::Result<()> {
    block_on(async {
        let (_dir, repo, pool) = fixture().await?;
        let exact = " \r\n大文字 'keep' \t";
        let capture = repo.capture(exact, 1).await?;
        let entry = repo.get(TextEntryId::Clipboard(capture)).await?;
        assert!(repo.promote(&entry, true).await.is_err()); // No key: no placeholder commit.
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM stickers")
            .fetch_one(&pool)
            .await?;
        assert_eq!(count, 0);
        assert!(repo.get(entry.id()).await.is_ok());
        let id = repo.promote(&entry, false).await?;
        let saved = repo.get(TextEntryId::Sticker(id)).await?;
        assert_eq!(saved.state(), Some(StickerState::Close));
        assert_eq!(&*repo.resolve(&saved, None).await?, exact);
        assert!(repo.get(entry.id()).await.is_err());
        assert!(repo.promote(&entry, false).await.is_err());
        assert!(repo.promote(&saved, false).await.is_err());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM stickers WHERE type='markdown'")
            .fetch_one(&pool)
            .await?;
        assert_eq!(count, 1);
        let evicted = repo.capture("evicted", 2).await?;
        let stale = repo.get(TextEntryId::Clipboard(evicted)).await?;
        repo.delete(stale.id()).await?;
        assert!(repo.promote(&stale, false).await.is_err());
        Ok(())
    })
}

#[test]
fn merged_order_filter_projection_and_typed_identity() -> anyhow::Result<()> {
    block_on(async {
        let (_dir, repo, pool) = fixture().await?;
        let s1 = sticker(&pool, "ALPHA", "body βeta", 10).await?;
        let s2 = sticker(&pool, "Other", "body βeta", 10).await?;
        let c1 = repo.capture("unique", 10).await?;
        let c2 = repo.capture("another", 10).await?;
        assert_eq!(s1, c1); // Same numeric ID, separate domain objects.
        let all = TextSources::new(true, true);
        let entries = repo.snapshot(all, "").await?;
        assert_eq!(
            entries.iter().map(|e| e.id()).collect::<Vec<_>>(),
            vec![
                TextEntryId::Sticker(s2),
                TextEntryId::Sticker(s1),
                TextEntryId::Clipboard(c2),
                TextEntryId::Clipboard(c1)
            ]
        );
        assert_eq!(filter_indices(&entries, "alpha\u{2003}ΒETA"), vec![1]);
        assert_eq!(
            repo.snapshot(all, "alpha βeta").await?[0].id(),
            TextEntryId::Sticker(s1)
        );
        assert!(repo.snapshot(all, "alpha absent").await?.is_empty());
        assert!(
            repo.snapshot(TextSources::new(false, false), "")
                .await?
                .is_empty()
        );
        let duplicate = repo.capture("body βeta", 50).await?;
        let merged = repo.snapshot(all, "").await?;
        assert_eq!(merged[0].id(), TextEntryId::Sticker(s1));
        assert_eq!(merged[0].activity_at(), 50);
        assert!(
            !merged
                .iter()
                .any(|e| e.id() == TextEntryId::Clipboard(duplicate))
        );
        assert!(repo.get(TextEntryId::Clipboard(duplicate)).await.is_ok());
        // Chrome updates don't edit text MRU; direct use is monotonic and table-specific.
        sqlx::query("UPDATE stickers SET updated_at=9999,color='blue' WHERE id=?")
            .bind(s1)
            .execute(&pool)
            .await?;
        assert_eq!(repo.get(TextEntryId::Sticker(s1)).await?.activity_at(), 10);
        repo.mark_used(TextEntryId::Clipboard(c1), 100).await?;
        repo.mark_used(TextEntryId::Clipboard(c1), 90).await?;
        assert_eq!(
            repo.get(TextEntryId::Clipboard(c1)).await?.activity_at(),
            100
        );
        assert_eq!(repo.get(TextEntryId::Sticker(s1)).await?.activity_at(), 10);
        repo.delete(TextEntryId::Clipboard(c1)).await?;
        assert!(repo.get(TextEntryId::Sticker(s1)).await.is_ok());
        for id in [TextEntryId::Sticker(s1), TextEntryId::Clipboard(c2)] {
            assert_eq!(id.to_string().parse::<TextEntryId>()?, id);
            assert_eq!(serde_json::to_value(id)?, id.to_string());
        }
        for invalid in ["1", "text:1", "sticker:0", "clipboard:-1", "sticker:x"] {
            assert!(invalid.parse::<TextEntryId>().is_err());
        }
        Ok(())
    })
}

#[test]
fn concurrent_connections_capture_and_promote_exactly_once() -> anyhow::Result<()> {
    block_on(async {
        let (dir, repo, pool) = fixture().await?;
        let other = TextRepository::open(dir.path().join("stickers.db")).await?;
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let worker_gate = gate.clone();
        let worker_repo = other.clone();
        let worker = std::thread::spawn(move || {
            worker_gate.wait();
            block_on(worker_repo.capture("simultaneous capture", 20))
        });
        gate.wait();
        let id = repo.capture("simultaneous capture", 10).await?;
        assert_eq!(worker.join().expect("capture worker panicked")?, id);
        assert_eq!(
            repo.get(TextEntryId::Clipboard(id)).await?.activity_at(),
            20
        );
        let selected = repo.get(TextEntryId::Clipboard(id)).await?;
        let worker_selected = selected.clone();
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let worker_gate = gate.clone();
        let worker = std::thread::spawn(move || {
            worker_gate.wait();
            block_on(other.promote(&worker_selected, false))
        });
        gate.wait();
        let local = repo.promote(&selected, false).await;
        let remote = worker.join().expect("promotion worker panicked");
        assert_eq!(
            usize::from(local.is_ok()) + usize::from(remote.is_ok()),
            1,
            "only one concurrent promotion may commit"
        );
        assert!(repo.get(selected.id()).await.is_err());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM stickers")
            .fetch_one(&pool)
            .await?;
        assert_eq!(count, 1);
        let saved = repo.snapshot(TextSources::new(true, false), "").await?;
        assert_eq!(saved[0].state(), Some(StickerState::Close));
        assert_eq!(
            &*repo.resolve(&saved[0], None).await?,
            "simultaneous capture"
        );
        Ok(())
    })
}

#[test]
fn failed_promotion_final_write_rolls_back_source_and_placeholder() -> anyhow::Result<()> {
    block_on(async {
        let (_dir, repo, pool) = fixture().await?;
        let capture = repo.capture("must survive", 10).await?;
        let source = repo.get(TextEntryId::Clipboard(capture)).await?;
        sqlx::raw_sql("CREATE TRIGGER reject_promotion BEFORE DELETE ON clipboard_captures BEGIN SELECT RAISE(ABORT,'injected final promotion failure'); END;")
            .execute(&pool).await?;
        assert!(repo.promote(&source, false).await.is_err());
        assert_eq!(&*repo.resolve(&source, None).await?, "must survive");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM stickers")
            .fetch_one(&pool)
            .await?;
        assert_eq!(
            count, 0,
            "failed source consumption must roll back the inserted sticker"
        );
        sqlx::query("DROP TRIGGER reject_promotion")
            .execute(&pool)
            .await?;
        assert!(repo.promote(&source, false).await.is_ok());
        Ok(())
    })
}

#[test]
fn stale_save_and_resolve_cannot_overwrite_new_content() -> anyhow::Result<()> {
    block_on(async {
        let (_dir, repo, pool) = fixture().await?;
        let id = sticker(&pool, "note", "original", 1).await?;
        let old = repo.get(TextEntryId::Sticker(id)).await?;
        let replacement = " \r\n新 'text'\t ";
        repo.save_text(&old, replacement).await?;
        assert!(repo.save_text(&old, "stale").await.is_err());
        assert!(repo.resolve(&old, None).await.is_err());
        let current = repo.get(old.id()).await?;
        assert_eq!(current.revision(), old.revision() + 1);
        assert!(current.activity_at() > old.activity_at());
        assert_eq!(&*repo.resolve(&current, None).await?, replacement);
        Ok(())
    })
}
