//! Integration tests against the real SQLite-backed clipboard store.
//!
//! The CLI and GUI both go through this storage layer, so verifying it as an isolated contract
//! gives us the confidence to plug the future GUI chooser on top without revalidating the
//! ordering/dedupe invariants.

#![cfg(feature = "storage")]

use futures::executor::block_on;
use rustickers::clipboard::items::{ClipboardItemKind, RetentionState};
use rustickers::clipboard::ordering::sort_in_place;
use rustickers::clipboard::storage::{ClipboardHistoryStore, SqliteClipboardStore};

#[test]
fn insert_then_snapshot_round_trips() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let store = block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).expect("open");

    let first = block_on(store.upsert_ordinary(
        "alpha content",
        "alpha content",
        &SqliteClipboardStore::ordinary_digest("alpha content"),
        13,
        1_700_000_000_000,
        None,
    ))
    .expect("insert first");
    let id_first = match first {
        rustickers::clipboard::storage::UpsertOutcome::Inserted { id } => id,
        _ => panic!("first insert should create"),
    };

    let second = block_on(store.upsert_ordinary(
        "beta content",
        "beta content",
        &SqliteClipboardStore::ordinary_digest("beta content"),
        12,
        1_700_000_001_000,
        None,
    ))
    .expect("insert second");
    let id_second = match second {
        rustickers::clipboard::storage::UpsertOutcome::Inserted { id } => id,
        _ => panic!("second insert should create"),
    };

    let snapshot = block_on(store.snapshot()).expect("snapshot");
    assert_eq!(snapshot.len(), 2);
    // MRU sort: bigger timestamp first; ties fall back to created_at + id, both deterministic.
    assert_eq!(snapshot[0].id, id_second);
    assert_eq!(snapshot[1].id, id_first);
    assert_eq!(snapshot[0].kind, ClipboardItemKind::Plain);
}

#[test]
fn digest_dedupe_returns_existing_row() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let store = block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).expect("open");

    let digest = SqliteClipboardStore::ordinary_digest("dup");
    let id1 = match block_on(store.upsert_ordinary("dup", "dup", &digest, 3, 1_700_000_000_000, None))
        .expect("first")
    {
        rustickers::clipboard::storage::UpsertOutcome::Inserted { id } => id,
        other => panic!("expected Inserted, got {other:?}"),
    };

    let id2 = match block_on(store.upsert_ordinary("dup", "dup", &digest, 3, 1_700_000_010_000, None))
        .expect("second")
    {
        rustickers::clipboard::storage::UpsertOutcome::Updated { id } => id,
        other => panic!("expected Updated, got {other:?}"),
    };
    assert_eq!(id1, id2);

    let snapshot = block_on(store.snapshot()).expect("snapshot");
    assert_eq!(snapshot.len(), 1, "no parallel rows for identical content");
    assert_eq!(
        snapshot[0].last_copied_at, 1_700_000_010_000,
        "duplicate copy bumps recency timestamp"
    );
}

#[test]
fn setting_retention_flags_protects_against_evict() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let store = block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).expect("open");

    let id = match block_on(store.upsert_ordinary(
        "pinned",
        "pinned",
        &SqliteClipboardStore::ordinary_digest("pinned"),
        "pinned".len() as i64,
        1_700_000_000_000,
        None,
    ))
    .expect("insert")
    {
        rustickers::clipboard::storage::UpsertOutcome::Inserted { id } => id,
        _ => panic!("insert"),
    };

    block_on(store.set_retention_flag(
        id,
        rustickers::clipboard::storage::RetentionFlag::Explicit,
        true,
    ))
    .expect("flag");

    let snapshot = block_on(store.snapshot()).expect("snapshot");
    assert!(snapshot[0].retention.explicit);
    assert!(snapshot[0].retention.is_retained());
    assert_eq!(
        snapshot[0].retention,
        RetentionState {
            explicit: true,
            edited: false,
            protected_: false
        }
    );
}

#[test]
fn filter_indices_match_against_plain_preview() {
    use rustickers::clipboard::filter::filter_indices;
    let snapshot = vec![
        make_row(1, "the quick brown fox", Some("the quick brown fox")),
        make_row(2, "another line", Some("another line")),
    ];
    let hits = filter_indices(&snapshot, "fox");
    assert_eq!(hits, vec![0]);
}

#[test]
fn sort_in_place_is_stable_across_modules() {
    let mut snapshot = Vec::new();
    snapshot.push(make_row(3, "z", Some("z")));
    snapshot.push(make_row(1, "a", Some("a")));
    snapshot.push(make_row(2, "m", Some("m")));
    sort_in_place(&mut snapshot);
    let ids: Vec<i64> = snapshot.iter().map(|row| row.id).collect();
    // Larger recency wins; helper makes id=1 newest, id=3 oldest: [1,2,3].
    assert_eq!(ids, vec![1, 2, 3]);
}

fn make_row(
    id: i64,
    content: &str,
    preview: Option<&str>,
) -> rustickers::clipboard::items::ClipboardItem {
    let now = chrono::Utc::now().timestamp_millis();
    rustickers::clipboard::items::ClipboardItem {
        id,
        kind: ClipboardItemKind::Plain,
        content: content.to_string(),
        plain_preview: preview.map(|p| p.to_owned()),
        public_label: None,
        byte_size: content.len() as i64,
        created_at: now - id,
        last_copied_at: now - id,
        last_used_at: None,
        last_edited_at: None,
        retention: RetentionState::default(),
    }
}

fn inserted_id(outcome: rustickers::clipboard::storage::UpsertOutcome) -> i64 {
    match outcome {
        rustickers::clipboard::storage::UpsertOutcome::Inserted { id }
        | rustickers::clipboard::storage::UpsertOutcome::Updated { id } => id,
    }
}

fn insert(store: &SqliteClipboardStore, text: &str, time: i64) -> i64 {
    inserted_id(
        block_on(store.upsert_ordinary(
            text,
            text,
            &SqliteClipboardStore::ordinary_digest(text),
            text.len() as i64,
            time,
            None,
        ))
        .unwrap(),
    )
}

#[test]
fn dedupe_preserves_exact_case_whitespace_and_edited_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let store = block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).unwrap();
    let id = insert(&store, "Alpha", 10);
    assert_ne!(id, insert(&store, "alpha", 11));
    assert_ne!(id, insert(&store, " Alpha ", 12));
    block_on(store.replace_content(id, "edited", 6, 13)).unwrap();
    assert_eq!(id, insert(&store, "edited", 14));
    assert_ne!(id, insert(&store, "Alpha", 15));
    assert_eq!(block_on(store.get(id)).unwrap().content, "edited");
}

#[test]
fn no_op_edit_does_not_retain_and_deleted_edit_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let store = block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).unwrap();
    let id = insert(&store, "same", 10);
    block_on(store.replace_content(id, "same", 4, 20)).unwrap();
    let row = block_on(store.get(id)).unwrap();
    assert!(!row.retention.edited);
    assert_eq!(row.last_edited_at, None);
    block_on(store.delete_row(id)).unwrap();
    assert!(block_on(store.replace_content(id, "lost", 4, 30)).is_err());
}

#[test]
fn concurrent_connections_dedupe_and_prune_revalidates_saved_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("clip.db");
    let a = block_on(SqliteClipboardStore::open(&path)).unwrap();
    let b = block_on(SqliteClipboardStore::open(&path)).unwrap();
    let worker = std::thread::spawn(move || insert(&b, "duplicate", 20));
    let id = insert(&a, "duplicate", 10);
    assert_eq!(id, worker.join().unwrap());
    assert_eq!(block_on(a.get(id)).unwrap().last_copied_at, 20);
    block_on(a.replace_content(id, "saved", 5, 30)).unwrap();
    block_on(a.delete_ordinary_by_ids(&[id])).unwrap();
    assert!(block_on(a.get(id)).unwrap().retention.edited);
}

#[test]
fn protected_storage_never_exposes_plaintext_and_cannot_orphan_vault() {
    use rustickers::clipboard::storage::RetentionFlag;
    use rustickers::clipboard::vault_controller::VaultController;
    use std::sync::Arc;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("clip.db");
    let store = Arc::new(block_on(SqliteClipboardStore::open(&path)).unwrap());
    let controller = VaultController::new(store.clone());
    block_on(controller.create("password")).unwrap();
    assert!(block_on(controller.unlock("password")).unwrap());
    let vault_envelope = controller
        .seal_for_storage(b"generation probe")
        .unwrap()
        .unwrap()
        .vault_envelope;
    let secret = "regression-secret-7791";
    let sealed = controller.seal(secret.as_bytes()).unwrap().unwrap();
    assert!(
        block_on(store.upsert_protected_bound(
            None,
            Some(secret),
            None,
            &sealed,
            22,
            10,
            &vault_envelope
        ))
        .is_err()
    );
    let id = inserted_id(
        block_on(store.upsert_protected_bound(
            Some("alias"),
            None,
            Some("keyed-token"),
            &sealed,
            22,
            10,
            &vault_envelope,
        ))
        .unwrap(),
    );
    let sealed_again = controller.seal(secret.as_bytes()).unwrap().unwrap();
    assert_eq!(
        id,
        inserted_id(
            block_on(store.upsert_protected_bound(
                None,
                None,
                Some("keyed-token"),
                &sealed_again,
                22,
                20,
                &vault_envelope
            ))
            .unwrap()
        )
    );
    let row = block_on(store.get(id)).unwrap();
    assert_eq!(row.kind, ClipboardItemKind::Protected);
    assert_eq!(row.content, sealed);
    assert_eq!(row.plain_preview, None);
    assert_eq!(row.searchable_string(), "alias\n");
    assert_eq!(
        controller.open(&row.content).unwrap().unwrap().as_slice(),
        secret.as_bytes()
    );
    assert!(block_on(store.replace_content(id, secret, secret.len() as i64, 30)).is_err());
    assert!(block_on(store.set_retention_flag(id, RetentionFlag::Protected, false)).is_err());
    block_on(store.set_retention_flag(id, RetentionFlag::Explicit, false)).unwrap();
    block_on(store.delete_ordinary_by_ids(&[id])).unwrap();
    assert!(block_on(store.get(id)).unwrap().retention.protected_);
    assert!(block_on(controller.create("replacement")).is_err());
    assert!(block_on(controller.disable()).is_err());
    assert!(
        block_on(store.upsert_ordinary(
            secret,
            secret,
            &SqliteClipboardStore::ordinary_digest(secret),
            secret.len() as i64,
            30,
            None,
        ))
        .is_err()
    );
    block_on(controller.rewrap("new-password")).unwrap();
    assert!(block_on(controller.unlock("new-password")).unwrap());
    assert_eq!(
        controller.open(&sealed).unwrap().unwrap().as_slice(),
        secret.as_bytes()
    );
    controller.lock();
    assert!(controller.open(&sealed).unwrap().is_none());
    // Check persisted columns, not just the redacted snapshot.
    let pool = block_on(sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
    ))
    .unwrap();
    let raw: (String, Option<String>, Option<String>) = block_on(
        sqlx::query_as(
            "SELECT content, plain_preview, plaintext_digest FROM clipboard_items WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(&pool),
    )
    .unwrap();
    assert_eq!(raw, (String::new(), None, None));
    assert!(
        !std::fs::read(path)
            .unwrap()
            .windows(secret.len())
            .any(|window| window == secret.as_bytes())
    );
}

#[test]
fn stale_password_change_cannot_overwrite_new_envelope() {
    use rustickers::clipboard::vault_controller::VaultController;
    use std::sync::Arc;
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).unwrap());
    let a = VaultController::new(store.clone());
    let b = VaultController::new(store.clone());
    block_on(a.create("old")).unwrap();
    assert!(block_on(a.unlock("old")).unwrap());
    assert!(block_on(b.unlock("old")).unwrap());
    block_on(a.rewrap("first")).unwrap();
    assert!(block_on(b.rewrap("stale")).is_err());
    assert!(block_on(a.unlock("first")).unwrap());
}

#[test]
fn stale_editor_save_fails_without_overwriting_newer_content() {
    let tmp = tempfile::tempdir().unwrap();
    let store = block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).unwrap();
    let id = insert(&store, "original", 10);
    block_on(store.compare_exchange_content(id, "original", "first", 5, 20, None)).unwrap();
    assert!(
        block_on(store.compare_exchange_content(id, "original", "stale", 5, 30, None)).is_err()
    );
    assert_eq!(block_on(store.get(id)).unwrap().content, "first");
    block_on(store.delete_row(id)).unwrap();
    assert!(block_on(store.compare_exchange_content(id, "first", "lost", 4, 40, None)).is_err());
}

#[test]
fn editor_save_writes_label_alongside_content() {
    let tmp = tempfile::tempdir().unwrap();
    let store = block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).unwrap();
    let id = insert(&store, "first", 5);

    // Edit content + label together; both must persist.
    block_on(store.compare_exchange_content(id, "first", "second", 6, 20, Some("alias"))).unwrap();
    let row = block_on(store.get(id)).unwrap();
    assert_eq!(row.content, "second");
    assert_eq!(row.public_label.as_deref(), Some("alias"));

    // Edit content while leaving the label untouched; the label survives.
    block_on(store.compare_exchange_content(id, "second", "third", 5, 30, None)).unwrap();
    let row = block_on(store.get(id)).unwrap();
    assert_eq!(row.content, "third");
    assert_eq!(row.public_label.as_deref(), Some("alias"));

    // Empty/whitespace label collapses to NULL.
    block_on(store.compare_exchange_content(id, "third", "fourth", 6, 40, Some("   "))).unwrap();
    let row = block_on(store.get(id)).unwrap();
    assert_eq!(row.content, "fourth");
    assert!(row.public_label.is_none());

    // A label-only change also commits: content moves with the new alias and
    // `last_edited_at` is bumped because the content differs.
    block_on(store.compare_exchange_content(id, "fourth", "fifth", 5, 50, Some("next-alias")))
        .unwrap();
    let row = block_on(store.get(id)).unwrap();
    assert_eq!(row.content, "fifth");
    assert_eq!(row.public_label.as_deref(), Some("next-alias"));
    assert_eq!(row.last_edited_at, Some(50));
    assert!(row.retention.edited);
}

#[test]
fn protected_display_helpers_ignore_even_damaged_plaintext_metadata() {
    let mut row = make_row(1, "secret-plaintext", Some("secret-preview"));
    row.kind = ClipboardItemKind::Protected;
    assert_eq!(row.preview_text(), "Protected item");
    assert_eq!(row.searchable_string(), "");
    row.public_label = Some("public alias".into());
    assert_eq!(row.preview_text(), "public alias");
    assert_eq!(row.searchable_string(), "public alias\n");
    let preview = rustickers::clipboard::ProtectedPreview {
        item: row,
        plaintext: "secret-plaintext".into(),
        match_token: Some("private-token".into()),
    };
    let debug = format!("{preview:?}");
    assert!(!debug.contains("secret"));
    assert!(!debug.contains("private-token"));
}

#[test]
fn ordinary_upsert_and_lowered_limits_trim_atomically_without_touching_retained() {
    use rustickers::clipboard::{ClipboardSettings, storage::RetentionFlag};
    let tmp = tempfile::tempdir().unwrap();
    let store = block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).unwrap();
    let saved = insert(&store, "saved", 1);
    block_on(store.set_retention_flag(saved, RetentionFlag::Explicit, true)).unwrap();
    let limits = ClipboardSettings {
        normal_count_limit: 2,
        normal_byte_limit: 8,
        capture_enabled: true,
    };
    block_on(store.update_settings(&limits)).unwrap();
    let old = insert(&store, "aaaa", 10);
    let current = insert(&store, "bbbb", 20);
    assert_eq!(block_on(store.snapshot()).unwrap().len(), 3);
    assert_eq!(current, insert(&store, "bbbb", 30));
    assert!(
        block_on(store.get(old)).is_ok(),
        "dedupe must not double count an incoming row"
    );
    let newest = insert(&store, "cccc", 40);
    assert!(block_on(store.get(old)).is_err());
    assert!(block_on(store.get(saved)).is_ok());
    let smaller = ClipboardSettings {
        normal_count_limit: 1,
        normal_byte_limit: 4,
        capture_enabled: true,
    };
    block_on(store.update_settings(&smaller)).unwrap();
    assert!(block_on(store.get(current)).is_err());
    assert!(block_on(store.get(newest)).is_ok());
    assert!(block_on(store.get(saved)).is_ok());
    assert!(
        block_on(store.upsert_ordinary(
            "oversized",
            "oversized",
            &SqliteClipboardStore::ordinary_digest("oversized"),
            9,
            50,
            None,
        ))
        .is_err()
    );
    assert!(
        block_on(store.get(newest)).is_ok(),
        "oversized insert must not prune old data"
    );
}

#[test]
fn protected_editor_cas_keeps_ciphertext_only_and_rejects_stale_save() {
    use rustickers::clipboard::vault_controller::VaultController;
    use std::sync::Arc;
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).unwrap());
    let controller = VaultController::new(store.clone());
    block_on(controller.create("password")).unwrap();
    assert!(block_on(controller.unlock("password")).unwrap());
    let vault_envelope = controller
        .seal_for_storage(b"generation probe")
        .unwrap()
        .unwrap()
        .vault_envelope;
    let first = controller.seal(b"first-secret").unwrap().unwrap();
    let id = inserted_id(
        block_on(store.upsert_protected_bound(None, None, None, &first, 12, 10, &vault_envelope))
            .unwrap(),
    );
    let second = controller.seal(b"edited-secret").unwrap().unwrap();
    block_on(store.compare_exchange_protected_content_bound(
        id,
        &first,
        &second,
        13,
        20,
        &vault_envelope,
        None,
    ))
    .unwrap();
    assert!(
        block_on(store.compare_exchange_protected_content_bound(
            id,
            &first,
            &first,
            12,
            30,
            &vault_envelope,
            None,
        ))
        .is_err()
    );
    let row = block_on(store.get(id)).unwrap();
    assert!(row.retention.edited && row.retention.protected_);
    assert!(
        !row.retention.explicit,
        "protection must not add an unrelated reservation reason"
    );
    assert_eq!(row.plain_preview, None);
    assert_eq!(
        controller.open(&row.content).unwrap().unwrap().as_slice(),
        b"edited-secret"
    );
}

#[test]
fn reset_generation_rejects_stale_insert_and_protection_without_mutation() {
    use rustickers::clipboard::vault_controller::VaultController;
    use std::sync::Arc;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("clip.db");
    let store = Arc::new(block_on(SqliteClipboardStore::open(&path)).unwrap());
    let other_store = Arc::new(block_on(SqliteClipboardStore::open(&path)).unwrap());
    let stale = VaultController::new(store.clone());
    let resetter = VaultController::new(other_store);
    let id = insert(&store, "ordinary", 10);
    assert!(!block_on(store.has_protected_items()).unwrap());
    block_on(stale.create("old-password")).unwrap();
    assert!(block_on(stale.unlock("old-password")).unwrap());
    let sealed = stale.seal_for_storage(b"ordinary").unwrap().unwrap();
    block_on(resetter.disable()).unwrap();
    // Reconfiguration happens through a different pool/controller; stale still holds its key.
    block_on(resetter.create("new-password")).unwrap();
    assert!(
        block_on(store.upsert_protected_bound(
            None,
            None,
            None,
            &sealed.ciphertext,
            8,
            20,
            &sealed.vault_envelope
        ))
        .is_err()
    );
    assert!(
        block_on(store.compare_exchange_protection(
            id,
            "ordinary",
            &sealed.ciphertext,
            &sealed.vault_envelope,
            None,
            None,
        ))
        .is_err()
    );
    assert!(block_on(store.upsert_protected(None, None, None, &sealed.ciphertext, 8, 20)).is_err());
    assert_eq!(block_on(store.get(id)).unwrap().content, "ordinary");
    assert_eq!(block_on(store.snapshot()).unwrap().len(), 1);
    assert!(!block_on(store.has_protected_items()).unwrap());
    assert!(block_on(resetter.unlock("new-password")).unwrap());
    let fresh = resetter.seal_for_storage(b"ordinary").unwrap().unwrap();
    block_on(store.compare_exchange_protection(
        id,
        "ordinary",
        &fresh.ciphertext,
        &fresh.vault_envelope,
        None,
        Some("  Saved command  "),
    ))
    .unwrap();
    assert!(block_on(store.has_protected_items()).unwrap());
    assert_eq!(
        block_on(store.get(id)).unwrap().public_label.as_deref(),
        Some("Saved command")
    );
    assert_eq!(
        resetter
            .open(&block_on(store.get(id)).unwrap().content)
            .unwrap()
            .unwrap()
            .as_slice(),
        b"ordinary"
    );
}

#[test]
fn protection_cas_preserves_identity_retention_and_recency_and_clears_live_plaintext() {
    use rustickers::clipboard::{storage::RetentionFlag, vault_controller::VaultController};
    use std::sync::Arc;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("clip.db");
    let store = Arc::new(block_on(SqliteClipboardStore::open(&path)).unwrap());
    let id = insert(&store, "original", 10);
    block_on(store.replace_content(id, "saved secret", 12, 20)).unwrap();
    block_on(store.set_retention_flag(id, RetentionFlag::Explicit, true)).unwrap();
    block_on(store.mark_used(id, 30)).unwrap();
    let before = block_on(store.get(id)).unwrap();
    let controller = VaultController::new(store.clone());
    block_on(controller.create("password")).unwrap();
    assert!(block_on(controller.unlock("password")).unwrap());
    let sealed = controller
        .seal_for_storage(before.content.as_bytes())
        .unwrap()
        .unwrap();
    assert!(
        block_on(store.compare_exchange_protection(
            id,
            "original",
            &sealed.ciphertext,
            &sealed.vault_envelope,
            None,
            None,
        ))
        .is_err()
    );
    block_on(store.compare_exchange_protection(
        id,
        &before.content,
        &sealed.ciphertext,
        &sealed.vault_envelope,
        None,
        None,
    ))
    .unwrap();
    let after = block_on(store.get(id)).unwrap();
    assert_eq!(after.id, before.id);
    assert_eq!(after.created_at, before.created_at);
    assert_eq!(after.last_copied_at, before.last_copied_at);
    assert_eq!(after.last_used_at, before.last_used_at);
    assert_eq!(after.last_edited_at, before.last_edited_at);
    assert!(after.retention.explicit && after.retention.edited && after.retention.protected_);
    assert_eq!(after.byte_size, before.byte_size);
    assert_eq!(after.plain_preview, None);
    assert_eq!(after.searchable_string(), "");
    assert_eq!(
        controller.open(&after.content).unwrap().unwrap().as_slice(),
        b"saved secret"
    );
    let pool = block_on(sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
    ))
    .unwrap();
    let raw: (String, Option<String>, Option<String>, Option<String>) = block_on(
        sqlx::query_as(
            "SELECT content, plain_preview, plaintext_digest, protected_match_token
            FROM clipboard_items WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(&pool),
    )
    .unwrap();
    assert_eq!(raw, (String::new(), None, None, None));
    // A stale repeat and a deleted selection cannot mutate or resurrect a protected row.
    assert!(
        block_on(store.compare_exchange_protection(
            id,
            &before.content,
            &sealed.ciphertext,
            &sealed.vault_envelope,
            None,
            None,
        ))
        .is_err()
    );
    block_on(store.delete_row(id)).unwrap();
    assert!(
        block_on(store.compare_exchange_protection(
            id,
            &before.content,
            &sealed.ciphertext,
            &sealed.vault_envelope,
            None,
            None,
        ))
        .is_err()
    );
}

#[test]
fn password_rewrap_requires_new_generation_for_protected_edit() {
    use rustickers::clipboard::vault_controller::VaultController;
    use std::sync::Arc;
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).unwrap());
    let controller = VaultController::new(store.clone());
    block_on(controller.create("old")).unwrap();
    assert!(block_on(controller.unlock("old")).unwrap());
    let first = controller.seal_for_storage(b"first").unwrap().unwrap();
    let id = inserted_id(
        block_on(store.upsert_protected_bound(
            None,
            None,
            None,
            &first.ciphertext,
            5,
            10,
            &first.vault_envelope,
        ))
        .unwrap(),
    );
    let stale_edit = controller.seal_for_storage(b"stale").unwrap().unwrap();
    block_on(controller.rewrap("new")).unwrap();
    assert!(
        block_on(store.compare_exchange_protected_content_bound(
            id,
            &first.ciphertext,
            &stale_edit.ciphertext,
            5,
            20,
            &stale_edit.vault_envelope,
            None,
        ))
        .is_err()
    );
    assert_eq!(block_on(store.get(id)).unwrap().content, first.ciphertext);
    assert!(block_on(controller.unlock("new")).unwrap());
    let fresh = controller.seal_for_storage(b"fresh").unwrap().unwrap();
    block_on(store.compare_exchange_protected_content_bound(
        id,
        &first.ciphertext,
        &fresh.ciphertext,
        5,
        20,
        &fresh.vault_envelope,
        None,
    ))
    .unwrap();
    assert_eq!(
        controller
            .open(&block_on(store.get(id)).unwrap().content)
            .unwrap()
            .unwrap()
            .as_slice(),
        b"fresh"
    );
}

#[test]
fn authenticated_unprotection_is_atomic_and_preserves_pin() {
    use rustickers::clipboard::storage::RetentionFlag;
    use rustickers::clipboard::vault_controller::VaultController;
    use std::sync::Arc;
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).unwrap());
    let vault = VaultController::new(store.clone());
    block_on(vault.create("p")).unwrap();
    assert!(block_on(vault.unlock("p")).unwrap());
    let sealed = vault.seal_for_storage(b"secret").unwrap().unwrap();
    let id = inserted_id(
        block_on(store.upsert_protected_bound(
            None,
            None,
            None,
            &sealed.ciphertext,
            6,
            10,
            &sealed.vault_envelope,
        ))
        .unwrap(),
    );
    block_on(store.set_retention_flag(id, RetentionFlag::Explicit, true)).unwrap();
    assert!(
        block_on(store.compare_exchange_unprotection(
            id,
            "stale",
            "secret",
            &sealed.vault_envelope
        ))
        .is_err()
    );
    assert!(
        block_on(store.compare_exchange_unprotection(
            id,
            &sealed.ciphertext,
            "secret",
            "stale-vault"
        ))
        .is_err()
    );
    assert_eq!(
        block_on(store.get(id)).unwrap().kind,
        ClipboardItemKind::Protected
    );
    block_on(store.compare_exchange_unprotection(
        id,
        &sealed.ciphertext,
        "secret",
        &sealed.vault_envelope,
    ))
    .unwrap();
    let row = block_on(store.get(id)).unwrap();
    assert_eq!(row.kind, ClipboardItemKind::Plain);
    assert_eq!(row.content, "secret");
    assert!(row.retention.explicit());
    assert!(!row.retention.protected());
    assert!(
        block_on(store.compare_exchange_unprotection(
            id,
            &sealed.ciphertext,
            "secret",
            &sealed.vault_envelope
        ))
        .is_err()
    );
}

#[test]
fn authorized_recapture_promotes_protected_row_and_lets_new_text_through() {
    use rustickers::clipboard::storage::UpsertOutcome;
    use rustickers::clipboard::vault_controller::VaultController;

    let tmp = tempfile::tempdir().expect("tmpdir");
    let store: std::sync::Arc<dyn ClipboardHistoryStore> =
        std::sync::Arc::new(block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).expect("open"));
    let controller = VaultController::new(store.clone());
    block_on(controller.create("password")).unwrap();
    assert!(block_on(controller.unlock("password")).unwrap());

    let secret = "recapture secret";
    let sealed = controller.seal_for_storage(secret.as_bytes()).unwrap().unwrap();
    let token = controller.match_token(secret.as_bytes()).unwrap().unwrap();
    let protected_id = inserted_id(
        block_on(store.upsert_protected_bound(
            None,
            None,
            Some(&token),
            &sealed.ciphertext,
            secret.len() as i64,
            10,
            &sealed.vault_envelope,
        ))
        .unwrap(),
    );

    // Without an authorized token the ordinary write still fails closed.
    assert!(block_on(store.upsert_ordinary(
        secret,
        secret,
        &SqliteClipboardStore::ordinary_digest(secret),
        secret.len() as i64,
        20,
        None
    ))
    .is_err());

    // Authorized and identical to the protected content: the protected row is promoted and no
    // plaintext copy is ever written.
    let outcome = block_on(store.upsert_ordinary(
        secret,
        secret,
        &SqliteClipboardStore::ordinary_digest(secret),
        secret.len() as i64,
        30,
        Some(&token),
    ))
    .unwrap();
    match outcome {
        UpsertOutcome::Updated { id } => assert_eq!(id, protected_id),
        other => panic!("expected promotion of the protected row, got {other:?}"),
    }
    assert_eq!(block_on(store.snapshot()).unwrap().len(), 1);
    assert!(block_on(store.get(protected_id)).unwrap().retention.protected_);

    // Authorized and different: the token proves absence, so ordinary collection proceeds.
    let fresh = "an ordinary later copy";
    let fresh_token = controller.match_token(fresh.as_bytes()).unwrap().unwrap();
    let outcome = block_on(store.upsert_ordinary(
        fresh,
        fresh,
        &SqliteClipboardStore::ordinary_digest(fresh),
        fresh.len() as i64,
        40,
        Some(&fresh_token),
    ))
    .unwrap();
    match outcome {
        UpsertOutcome::Inserted { .. } => {}
        other => panic!("expected Inserted for non-matching text, got {other:?}"),
    }
    assert_eq!(block_on(store.snapshot()).unwrap().len(), 2);
}

#[test]
fn protecting_a_row_stores_its_token_for_later_recapture() {
    use rustickers::clipboard::vault_controller::VaultController;

    let tmp = tempfile::tempdir().expect("tmpdir");
    let store: std::sync::Arc<dyn ClipboardHistoryStore> =
        std::sync::Arc::new(block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).expect("open"));
    let text = "about to be protected";
    let id = {
        use rustickers::clipboard::storage::UpsertOutcome;
        match block_on(store.upsert_ordinary(
            text,
            text,
            &SqliteClipboardStore::ordinary_digest(text),
            text.len() as i64,
            10,
            None,
        ))
        .unwrap()
        {
            UpsertOutcome::Inserted { id } => id,
            other => panic!("{other:?}"),
        }
    };
    let controller = VaultController::new(store.clone());
    block_on(controller.create("password")).unwrap();
    assert!(block_on(controller.unlock("password")).unwrap());
    let sealed = controller.seal_for_storage(text.as_bytes()).unwrap().unwrap();
    let token = controller.match_token(text.as_bytes()).unwrap().unwrap();
    block_on(store.compare_exchange_protection(
        id,
        text,
        &sealed.ciphertext,
        &sealed.vault_envelope,
        Some(&token),
        None,
    ))
    .unwrap();

    // The token survived protecting, so an authorized recapture of the same text promotes the
    // protected row instead of writing plaintext back.
    let outcome = block_on(store.upsert_ordinary(
        text,
        text,
        &SqliteClipboardStore::ordinary_digest(text),
        text.len() as i64,
        20,
        Some(&token),
    ))
    .unwrap();
    assert!(matches!(outcome, rustickers::clipboard::storage::UpsertOutcome::Updated { id: promoted } if promoted == id));
    assert_eq!(block_on(store.get(id)).unwrap().content, sealed.ciphertext);
}
