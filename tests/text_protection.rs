#![cfg(feature = "storage")]

use anyhow::Context as _;
use futures::executor::block_on;
use rustickers::text::{
    CaptureOutcome, TextEntry, TextEntryId, TextRepository, TextSources,
    protection::{self, CONTENT_PREFIX, MasterKey},
};
use sha2::{Digest, Sha256};
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
async fn promote(repo: &TextRepository, text: &str, encrypted: bool) -> anyhow::Result<TextEntry> {
    let id = repo
        .capture(text, 1)
        .await?
        .capture_id()
        .context("expected a fresh clipboard capture")?;
    let source = repo.get(TextEntryId::Clipboard(id)).await?;
    let saved = repo.promote(&source, encrypted).await?;
    repo.get(TextEntryId::Sticker(saved)).await
}
async fn content(pool: &SqlitePool, id: TextEntryId) -> anyhow::Result<String> {
    let TextEntryId::Sticker(id) = id else {
        panic!("expected sticker")
    };
    Ok(
        sqlx::query_scalar("SELECT content FROM stickers WHERE id=?")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}
async fn persisted(pool: &SqlitePool) -> anyhow::Result<serde_json::Value> {
    // Capture all columns, not just ciphertext: rollback must restore revisions and metadata too.
    let rows: Vec<String> = sqlx::query_scalar("SELECT json_object('id',id,'title',title,'state',state,'left',left,'top',top,'width',width,'height',height,'color',color,'content',content,'created',created_at,'updated',updated_at,'modified',text_modified_at,'used',text_last_used_at,'revision',text_revision,'digest',text_digest) FROM stickers ORDER BY id")
        .fetch_all(pool).await?;
    let key: String =
        sqlx::query_scalar("SELECT configuration FROM text_master_key WHERE singleton=1")
            .fetch_one(pool)
            .await?;
    Ok(serde_json::json!({"rows": rows, "key": key}))
}

#[test]
fn rotation_backfills_legacy_locked_digest_and_consumes_plain_capture() -> anyhow::Result<()> {
    block_on(async {
        let (dir, repo, pool) = fixture().await?;
        repo.set_master_password("old password").await?;
        let locked = promote(&repo, "legacy locked secret", true).await?;
        let TextEntryId::Sticker(id) = locked.id() else {
            unreachable!()
        };
        sqlx::query("UPDATE stickers SET text_digest='' WHERE id=?")
            .bind(id)
            .execute(&pool)
            .await?;
        let reopened = TextRepository::open(dir.path().join("stickers.db")).await?;
        let digest: String = sqlx::query_scalar("SELECT text_digest FROM stickers WHERE id=?")
            .bind(id)
            .fetch_one(&pool)
            .await?;
        assert!(
            digest.is_empty(),
            "startup must not hash ciphertext or decrypt"
        );
        let capture = reopened
            .capture("legacy locked secret", 100)
            .await?
            .capture_id()
            .unwrap();
        reopened
            .change_master_password("old password", "new password")
            .await?;
        assert!(reopened.get(TextEntryId::Clipboard(capture)).await.is_err());
        assert_eq!(
            reopened
                .capture("legacy locked secret", 200)
                .await?
                .sticker_id(),
            Some(id)
        );
        assert_eq!(
            reopened
                .resolve(&reopened.get(locked.id()).await?, Some("new password"))
                .await?
                .as_str(),
            "legacy locked secret"
        );
        Ok(())
    })
}

#[test]
fn locked_edit_consumes_matching_capture_and_redacts_digest() -> anyhow::Result<()> {
    block_on(async {
        let (_dir, repo, pool) = fixture().await?;
        repo.set_master_password("password").await?;
        let locked = promote(&repo, "original secret", true).await?;
        let capture = repo
            .capture("replacement secret", 100)
            .await?
            .capture_id()
            .unwrap();
        repo.save_text(&locked, "replacement secret").await?;
        assert!(repo.get(TextEntryId::Clipboard(capture)).await.is_err());
        let current = repo.get(locked.id()).await?;
        let json = serde_json::to_value(&current)?;
        assert!(
            json.get("digest").is_none(),
            "redacted snapshots must not publish fingerprints"
        );
        let TextEntryId::Sticker(id) = current.id() else {
            unreachable!()
        };
        let digest: String = sqlx::query_scalar("SELECT text_digest FROM stickers WHERE id=?")
            .bind(id)
            .fetch_one(&pool)
            .await?;
        assert_eq!(
            digest,
            format!("{:x}", Sha256::digest(b"replacement secret"))
        );
        assert_eq!(
            repo.capture("replacement secret", 200).await?.sticker_id(),
            Some(id)
        );
        Ok(())
    })
}

#[test]
fn public_only_seal_exact_open_and_fail_closed_formats() -> anyhow::Result<()> {
    let key = MasterKey::create("correct password")?;
    let text = " \t秘密\r\n'quotes' \"double\";\n ";
    let sealed = key.seal(7, text)?;
    assert!(protection::is_encrypted(&sealed));
    assert!(!sealed.contains(text));
    assert_eq!(&*key.open(7, &sealed, "correct password")?, text);
    assert!(key.open(7, &sealed, "wrong password").is_err());
    assert!(key.open(8, &sealed, "correct password").is_err());
    assert!(
        key.open(
            7,
            &format!("{CONTENT_PREFIX}{}\ninvalid!", key.key_id()),
            "correct password"
        )
        .is_err()
    );
    assert!(
        key.open(
            7,
            &sealed.replace(key.key_id(), "different-generation"),
            "correct password"
        )
        .is_err()
    );
    assert!(
        key.open(7, "RUSTICKERS_TEXT_V2\nanything", "correct password")
            .is_err()
    );
    assert!(MasterKey::create("").is_err());
    assert!(MasterKey::parse("").is_err());
    let mut configuration: serde_json::Value = serde_json::from_str(&key.serialize()?)?;
    // Unusable wrapped private identity proves sealing does not authorize or decrypt it.
    configuration["wrapped_identity"] = serde_json::json!("");
    let public_only = MasterKey::parse(&configuration.to_string())?;
    let public_sealed = public_only.seal(7, text)?;
    assert_eq!(&*key.open(7, &public_sealed, "correct password")?, text);
    assert!(public_only.verify_password("correct password").is_err());
    for field in ["version", "memory_kib", "iterations", "lanes"] {
        let mut invalid = configuration.clone();
        invalid[field] = serde_json::json!(0);
        assert!(MasterKey::parse(&invalid.to_string()).is_err(), "{field}");
    }
    for value in [
        "RUSTICKERS_TEXT_V2\nx",
        " RUSTICKERS_LOCKED_V1\nx",
        "RUSTICKERS_VAULT_V1\nx",
    ] {
        assert!(protection::is_unsupported(value));
    }
    Ok(())
}

#[test]
fn encrypted_promotion_lock_save_redaction_and_explicit_remove_encryption() -> anyhow::Result<()> {
    block_on(async {
        let (_dir, repo, pool) = fixture().await?;
        repo.set_master_password("password").await?;
        assert!(repo.set_master_password("replacement").await.is_err());
        let secret = "secret 大文字\r\n'keep' ";
        let locked_secret = "locked 大文字 'leak' ";
        let plain_sticker = promote(&repo, secret, false).await?;
        let locked_entry = promote(&repo, locked_secret, true).await?;
        // The two stickers hold different plaintext: an identical capture would
        // be deduplicated into the first one instead of promoting twice.
        let plain_sticker_id = match plain_sticker.id() {
            TextEntryId::Sticker(id) => id,
            TextEntryId::Clipboard(_) => unreachable!("plain promote returns a sticker"),
        };
        let locked_sticker_id = match locked_entry.id() {
            TextEntryId::Sticker(id) => id,
            TextEntryId::Clipboard(_) => unreachable!("encrypted promote returns a sticker"),
        };
        assert_ne!(plain_sticker_id, locked_sticker_id);
        let encrypted = repo.get(locked_entry.id()).await?;
        assert!(encrypted.is_encrypted());
        assert_eq!(encrypted.title(), "Locked text");
        assert_eq!(encrypted.preview(), "Locked text");
        assert!(
            repo.snapshot(TextSources::new(false, true), "")
                .await?
                .is_empty()
        );
        // The plainly stored sticker lives side-by-side with a sealed sticker
        // holding a different plaintext, exercising the projection paths the new
        // digest index enables.
        assert!(!plain_sticker.is_encrypted());
        let before = content(&pool, encrypted.id()).await?;
        assert!(before.starts_with(CONTENT_PREFIX));
        assert!(!before.contains(locked_secret));
        let json = serde_json::to_string(&encrypted)?;
        assert!(!json.contains(locked_secret));
        assert!(!json.contains(CONTENT_PREFIX));
        assert!(!format!("{encrypted:?}").contains(locked_secret));
        assert!(!encrypted.matches(locked_secret));
        assert!(encrypted.matches("locked"));
        assert!(repo.resolve(&encrypted, None).await.is_err());
        assert!(repo.resolve(&encrypted, Some("wrong")).await.is_err());
        assert_eq!(
            &*repo.resolve(&encrypted, Some("password")).await?,
            locked_secret
        );
        assert_eq!(content(&pool, encrypted.id()).await?, before); // Temporary unlock is read-only.
        assert_eq!(&*repo.resolve(&plain_sticker, None).await?, secret);
        let replacement = "edited locked text\r\n新 ";
        repo.save_text(&encrypted, replacement).await?; // No private key/password needed.
        assert!(repo.save_text(&encrypted, "stale").await.is_err());
        let edited = repo.get(encrypted.id()).await?;
        assert!(edited.is_encrypted());
        assert!(!content(&pool, edited.id()).await?.contains(replacement));
        assert_eq!(edited.title(), "Locked text");
        assert_eq!(
            &*repo.resolve(&edited, Some("password")).await?,
            replacement
        );
        assert!(
            repo.set_encryption(&edited, false, Some("wrong"))
                .await
                .is_err()
        );
        repo.set_encryption(&edited, false, Some("password"))
            .await?;
        assert_eq!(content(&pool, edited.id()).await?, replacement);
        let stale_plain = repo.get(plain_sticker.id()).await?;
        let copied_at = stale_plain.activity_at() + 1_000;
        assert_eq!(
            repo.capture(secret, copied_at).await?,
            CaptureOutcome::RefreshedSticker(plain_sticker_id)
        );
        // Recopying identical text must not introduce a fresh clipboard row;
        // the saved plaintext sticker already absorbs the activity.
        let row: Option<i64> = sqlx::query_scalar("SELECT id FROM clipboard_captures WHERE text=?")
            .bind(secret)
            .fetch_optional(&pool)
            .await?;
        assert!(
            row.is_none(),
            "no plaintext capture row should leak next to the saved sticker"
        );
        repo.set_encryption(&stale_plain, true, None).await?;
        assert!(
            repo.save_text(&stale_plain, "plaintext resurrection")
                .await
                .is_err()
        );
        assert!(
            repo.snapshot(TextSources::new(false, true), "")
                .await?
                .is_empty()
        );
        // The pre-encryption recency is preserved (`set_encryption` does not
        // touch text_modified_at or text_last_used_at). The new dedup path
        // does advance the saved sticker's recency, so the post-encryption
        // activity should equal what the duplicate capture just installed.
        assert_eq!(repo.get(plain_sticker.id()).await?.activity_at(), copied_at);
        assert_eq!(repo.get(plain_sticker.id()).await?.revision(), 1);
        Ok(())
    })
}

#[test]
fn row_swaps_are_rejected_and_corrupt_configuration_never_promotes_plaintext() -> anyhow::Result<()>
{
    block_on(async {
        let (_dir, repo, pool) = fixture().await?;
        repo.set_master_password("password").await?;
        let a = promote(&repo, "first secret", true).await?;
        let b = promote(&repo, "second secret", true).await?;
        let ca = content(&pool, a.id()).await?;
        let cb = content(&pool, b.id()).await?;
        let TextEntryId::Sticker(aid) = a.id() else {
            unreachable!()
        };
        let TextEntryId::Sticker(bid) = b.id() else {
            unreachable!()
        };
        sqlx::query(
            "UPDATE stickers SET content=CASE id WHEN ? THEN ? WHEN ? THEN ? END WHERE id IN (?,?)",
        )
        .bind(aid)
        .bind(cb)
        .bind(bid)
        .bind(ca)
        .bind(aid)
        .bind(bid)
        .execute(&pool)
        .await?;
        for id in [a.id(), b.id()] {
            let swapped = repo.get(id).await?;
            assert!(repo.resolve(&swapped, Some("password")).await.is_err());
        }
        sqlx::query("UPDATE text_master_key SET configuration='{}'")
            .execute(&pool)
            .await?;
        let capture = repo
            .capture("must stay capture", 10)
            .await?
            .capture_id()
            .context("expected a fresh clipboard capture")?;
        let source = repo.get(TextEntryId::Clipboard(capture)).await?;
        assert!(repo.promote(&source, true).await.is_err());
        assert!(repo.get(source.id()).await.is_ok());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM stickers")
            .fetch_one(&pool)
            .await?;
        assert_eq!(count, 2);
        Ok(())
    })
}

#[test]
fn complete_rotation_rewrites_all_payloads_preserves_metadata_and_rejects_stale_save()
-> anyhow::Result<()> {
    block_on(async {
        let (_dir, repo, pool) = fixture().await?;
        repo.set_master_password("old password").await?;
        let a = promote(&repo, "first\r\n秘密 ", true).await?;
        let b = promote(&repo, "second\t'quoted'", true).await?;
        let plain = promote(&repo, "ordinary", false).await?;
        let before = persisted(&pool).await?;
        let old_key = repo.master_key().await?.unwrap();
        let old_contents = [content(&pool, a.id()).await?, content(&pool, b.id()).await?];
        repo.change_master_password("old password", "new password")
            .await?;
        let new_key = repo.master_key().await?.unwrap();
        assert_ne!(old_key.key_id(), new_key.key_id());
        let after = persisted(&pool).await?;
        for (index, stale) in [a, b].iter().enumerate() {
            let current = repo.get(stale.id()).await?;
            let ciphertext = content(&pool, current.id()).await?;
            assert_ne!(ciphertext, old_contents[index]);
            assert!(ciphertext.contains(new_key.key_id()));
            assert_eq!(current.revision(), stale.revision() + 1);
            assert_eq!(current.activity_at(), stale.activity_at());
            assert!(repo.resolve(&current, Some("old password")).await.is_err());
            let exact = if index == 0 {
                "first\r\n秘密 "
            } else {
                "second\t'quoted'"
            };
            assert_eq!(&*repo.resolve(&current, Some("new password")).await?, exact);
            assert!(repo.save_text(stale, "stale draft").await.is_err());
            let TextEntryId::Sticker(id) = stale.id() else {
                unreachable!()
            };
            assert!(old_key.open(id, &ciphertext, "old password").is_err());
            let mut old: serde_json::Value =
                serde_json::from_str(before["rows"][index].as_str().unwrap())?;
            let mut new: serde_json::Value =
                serde_json::from_str(after["rows"][index].as_str().unwrap())?;
            for row in [&mut old, &mut new] {
                row.as_object_mut().unwrap().remove("content");
                row.as_object_mut().unwrap().remove("revision");
            }
            assert_eq!(old, new);
        }
        assert_eq!(repo.get(plain.id()).await?.revision(), plain.revision());
        assert_eq!(
            &*repo.resolve(&repo.get(plain.id()).await?, None).await?,
            "ordinary"
        );
        assert_eq!(before["rows"][2], after["rows"][2]);
        Ok(())
    })
}

#[test]
fn unsupported_stickers_are_unavailable_and_damaged_current_entries_are_redacted()
-> anyhow::Result<()> {
    block_on(async {
        let (_dir, repo, pool) = fixture().await?;
        repo.set_master_password("password").await?;
        for envelope in [
            "RUSTICKERS_LOCKED_V1\nlegacy-secret",
            " RUSTICKERS_VAULT_V1\nlegacy-secret",
            "RUSTICKERS_TEXT_V2\nlegacy-secret",
        ] {
            let id: i64 = sqlx::query_scalar("INSERT INTO stickers(title,state,left,top,width,height,color,type,content,created_at,updated_at) VALUES('legacy title','close',0,0,400,300,'yellow','markdown',?,1,1) RETURNING id")
                .bind(envelope).fetch_one(&pool).await?;
            assert!(repo.get(TextEntryId::Sticker(id)).await.is_err());
        }
        let damaged = format!(
            "{CONTENT_PREFIX}{}\nprivate-payload-marker!",
            repo.master_key().await?.unwrap().key_id()
        );
        let id: i64 = sqlx::query_scalar("INSERT INTO stickers(title,state,left,top,width,height,color,type,content,created_at,updated_at) VALUES('Public alias','close',0,0,400,300,'yellow','markdown',?,1,1) RETURNING id")
            .bind(&damaged).fetch_one(&pool).await?;
        let rows = repo.snapshot(TextSources::new(true, true), "").await?;
        assert_eq!(rows.len(), 1);
        let entry = &rows[0];
        assert_eq!(entry.id(), TextEntryId::Sticker(id));
        assert!(entry.is_encrypted());
        assert_eq!(entry.preview(), "Public alias");
        assert!(entry.matches("public alias"));
        assert!(entry.matches(&entry.id().to_string()));
        assert!(!entry.matches("private-payload-marker"));
        let json = serde_json::to_string(entry)?;
        let debug = format!("{entry:?}");
        for output in [json, debug] {
            assert!(!output.contains(CONTENT_PREFIX));
            assert!(!output.contains("private-payload-marker"));
            assert!(!output.contains("legacy-secret"));
        }
        let error = repo.resolve(entry, Some("password")).await.unwrap_err();
        assert!(!format!("{error:#}").contains("private-payload-marker"));
        assert_eq!(content(&pool, entry.id()).await?, damaged);
        Ok(())
    })
}

#[test]
fn rotation_final_write_failure_restores_every_row_and_key() -> anyhow::Result<()> {
    block_on(async {
        let (_dir, repo, pool) = fixture().await?;
        repo.set_master_password("old password").await?;
        let first = promote(&repo, "first intact secret", true).await?;
        let second = promote(&repo, "second intact secret", true).await?;
        let before = persisted(&pool).await?;
        sqlx::raw_sql("CREATE TRIGGER reject_new_key BEFORE UPDATE ON text_master_key BEGIN SELECT RAISE(ABORT,'injected key write failure'); END;")
            .execute(&pool).await?;
        assert!(
            repo.change_master_password("old password", "new password")
                .await
                .is_err()
        );
        assert_eq!(
            persisted(&pool).await?,
            before,
            "failure after ciphertext rewrites must roll back the whole transaction"
        );
        for (entry, exact) in [
            (&first, "first intact secret"),
            (&second, "second intact secret"),
        ] {
            assert_eq!(&*repo.resolve(entry, Some("old password")).await?, exact);
        }
        Ok(())
    })
}

#[test]
fn concurrent_rotations_have_one_winner_and_stale_cross_pool_editor_cannot_write()
-> anyhow::Result<()> {
    block_on(async {
        let (dir, repo, pool) = fixture().await?;
        repo.set_master_password("old password").await?;
        let stale = promote(&repo, "rotation race secret", true).await?;
        let other = TextRepository::open(dir.path().join("stickers.db")).await?;
        let remote_stale = other.get(stale.id()).await?;
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let worker_gate = gate.clone();
        let worker_repo = other.clone();
        let worker = std::thread::spawn(move || {
            worker_gate.wait();
            block_on(worker_repo.change_master_password("old password", "remote password"))
        });
        gate.wait();
        let local = repo
            .change_master_password("old password", "local password")
            .await;
        let remote = worker.join().expect("rotation worker panicked");
        assert_eq!(
            usize::from(local.is_ok()) + usize::from(remote.is_ok()),
            1,
            "concurrent old-password rotations must not overwrite each other's generation"
        );
        let winner = if local.is_ok() {
            "local password"
        } else {
            "remote password"
        };
        let loser = if local.is_ok() {
            "remote password"
        } else {
            "local password"
        };
        let before_stale = persisted(&pool).await?;
        assert!(
            other
                .save_text(&remote_stale, "stale plaintext resurrection")
                .await
                .is_err()
        );
        assert!(
            other
                .set_encryption(&remote_stale, false, Some("old password"))
                .await
                .is_err()
        );
        assert!(
            other
                .resolve(&remote_stale, Some("old password"))
                .await
                .is_err()
        );
        assert_eq!(persisted(&pool).await?, before_stale);
        let current = repo.get(stale.id()).await?;
        assert_eq!(current.revision(), stale.revision() + 1);
        assert_eq!(
            &*repo.resolve(&current, Some(winner)).await?,
            "rotation race secret"
        );
        assert!(repo.resolve(&current, Some(loser)).await.is_err());
        assert!(repo.resolve(&current, Some("old password")).await.is_err());
        // Collection remains ordinary and independent of encryption/private-key authorization.
        let capture = other
            .capture("external later secret copy", 100)
            .await?
            .capture_id()
            .context("expected a fresh clipboard capture")?;
        let entry = other.get(TextEntryId::Clipboard(capture)).await?;
        assert_eq!(
            &*other.resolve(&entry, None).await?,
            "external later secret copy"
        );
        Ok(())
    })
}

#[test]
fn encrypted_promotion_racing_rotation_never_commits_old_generation_payload() -> anyhow::Result<()>
{
    block_on(async {
        let (dir, repo, pool) = fixture().await?;
        repo.set_master_password("old password").await?;
        let existing = promote(&repo, "existing secret", true).await?;
        let source_id = repo
            .capture("concurrently promoted secret", 10)
            .await?
            .capture_id()
            .context("expected a fresh clipboard capture")?;
        let source = repo.get(TextEntryId::Clipboard(source_id)).await?;
        let other = TextRepository::open(dir.path().join("stickers.db")).await?;
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let worker_gate = gate.clone();
        let worker = std::thread::spawn(move || {
            worker_gate.wait();
            block_on(other.promote(&source, true))
        });
        gate.wait();
        repo.change_master_password("old password", "new password")
            .await?;
        let saved_id = worker.join().expect("promotion worker panicked")?;
        let key = repo.master_key().await?.unwrap();
        assert!(repo.get(TextEntryId::Clipboard(source_id)).await.is_err());
        for (id, exact) in [
            (existing.id(), "existing secret"),
            (
                TextEntryId::Sticker(saved_id),
                "concurrently promoted secret",
            ),
        ] {
            let current = repo.get(id).await?;
            assert!(current.is_encrypted());
            assert!(content(&pool, id).await?.contains(key.key_id()));
            assert_eq!(&*repo.resolve(&current, Some("new password")).await?, exact);
            assert!(repo.resolve(&current, Some("old password")).await.is_err());
        }
        assert_eq!(
            repo.snapshot(TextSources::new(true, false), "")
                .await?
                .len(),
            2
        );
        Ok(())
    })
}

#[test]
fn damaged_second_item_and_wrong_password_roll_back_entire_rotation() -> anyhow::Result<()> {
    block_on(async {
        let (_dir, repo, pool) = fixture().await?;
        repo.set_master_password("old password").await?;
        let first = promote(&repo, "first intact secret", true).await?;
        let second = promote(&repo, "second secret", true).await?;
        let before = persisted(&pool).await?;
        assert!(
            repo.change_master_password("wrong", "new password")
                .await
                .is_err()
        );
        assert_eq!(persisted(&pool).await?, before);
        let TextEntryId::Sticker(id) = second.id() else {
            unreachable!()
        };
        let damaged = format!(
            "{CONTENT_PREFIX}{}\nnot-valid-base64!",
            repo.master_key().await?.unwrap().key_id()
        );
        sqlx::query("UPDATE stickers SET content=? WHERE id=?")
            .bind(damaged)
            .bind(id)
            .execute(&pool)
            .await?;
        let before_failure = persisted(&pool).await?;
        assert!(
            repo.change_master_password("old password", "new password")
                .await
                .is_err()
        );
        assert_eq!(persisted(&pool).await?, before_failure);
        assert_eq!(
            &*repo.resolve(&first, Some("old password")).await?,
            "first intact secret"
        );
        assert!(
            repo.master_key()
                .await?
                .unwrap()
                .verify_password("new password")
                .is_err()
        );
        assert!(
            repo.resolve(&repo.get(second.id()).await?, Some("old password"))
                .await
                .is_err()
        );
        Ok(())
    })
}

/// Same-text clipboard capture must refresh the existing saved sticker's
/// recency without ever materialising a duplicate capture row, and the
/// sticker involved may be either plaintext or already locked — the digest
/// is computed at seal time so the two cases share the same identity.
#[test]
fn clipboard_capture_with_matching_save_refreshes_not_duplicates() -> anyhow::Result<()> {
    block_on(async {
        let (_dir, repo, pool) = fixture().await?;
        repo.set_master_password("password").await?;
        let secret = "secret 大文字\r\n'keep' ";
        let independent = promote(&repo, secret, false).await?;
        let tracked_id = independent.id();
        let before_recency: (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(text_last_used_at,0),text_modified_at FROM stickers WHERE id=?",
        )
        .bind(match tracked_id {
            TextEntryId::Sticker(id) => id,
            TextEntryId::Clipboard(_) => unreachable!(),
        })
        .fetch_one(&pool)
        .await?;
        // Lock the sticker; the digest should now match the plaintext we
        // capture below, exercising the cross-format dedup path.
        repo.set_encryption(&independent, true, None).await?;
        // Recopy identical text while it's already a sealed sticker.
        let copied_at = 1_700_000_000_000;
        let outcome = repo.capture(secret, copied_at).await?;
        let refreshed = outcome
            .sticker_id()
            .context("expected the duplicate to refresh the saved sticker")?;
        let TextEntryId::Sticker(tracked_sticker) = tracked_id else {
            unreachable!()
        };
        assert_eq!(refreshed, tracked_sticker);
        let after: (i64, i64, String) = sqlx::query_as(
            "SELECT COALESCE(text_last_used_at,0),text_modified_at,content FROM stickers WHERE id=?",
        )
        .bind(match tracked_id {
            TextEntryId::Sticker(id) => id,
            TextEntryId::Clipboard(_) => unreachable!(),
        })
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            after.0, copied_at,
            "text_last_used_at jumps to the new capture time"
        );
        assert_eq!(
            after.1, before_recency.1,
            "text_modified_at remains stable: encryption is a protection flip, not a content edit"
        );
        assert!(
            after.2.starts_with(CONTENT_PREFIX),
            "sticker stays locked; the duplicate capture must not silently unlock it"
        );
        let captures: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM clipboard_captures")
            .fetch_one(&pool)
            .await?;
        assert_eq!(
            captures, 0,
            "the duplicate must not produce a plaintext capture row next to a locked sticker"
        );
        // After deleting the saved sticker, a fresh capture of the same text
        // re-creates an ordinary clipboard row (the path through which new
        // copies land once the prior matching record is gone).
        repo.delete(tracked_id).await?;
        let stored = repo
            .capture(secret, copied_at + 1)
            .await?
            .capture_id()
            .context("expected a stored capture row after the saved sticker was deleted")?;
        let row: (i64, String) =
            sqlx::query_as("SELECT id,text FROM clipboard_captures WHERE id=?")
                .bind(stored)
                .fetch_one(&pool)
                .await?;
        assert_eq!(row.1, secret);
        Ok(())
    })
}
