use rustickers::model::sticker::*;
use rustickers::storage::StickerStore;
use rustickers::storage::sqlite::SqliteStore;

fn detail(sticker_type: StickerType, state: StickerState, content: &str) -> StickerDetail {
    StickerDetail {
        id: 0,
        title: "t".into(),
        state,
        left: 10,
        top: 20,
        width: 210,
        height: 114,
        top_most: false,
        color: StickerColor::Yellow,
        sticker_type,
        content: content.into(),
        created_at: 0,
        updated_at: 0,
        display_id: None,
        display_uuid: None,
        virtual_desktop_id: None,
        native_left: None,
        native_top: None,
        native_width: None,
        native_height: None,
        preferred_display_uuid: None,
        placements: Vec::new(),
    }
}

/// Each test gets a unique, ownership-scoped database directory.
struct TempDb(tempfile::TempDir);

impl TempDb {
    fn new(name: &str) -> Self {
        Self(
            tempfile::Builder::new()
                .prefix(&format!("rustickers-manager-{name}-"))
                .tempdir()
                .unwrap(),
        )
    }

    fn path(&self) -> std::path::PathBuf {
        self.0.path().join("stickers.db")
    }
}

#[test]
fn command_stickers_exclude_non_command_types() {
    let db = TempDb::new("exclude");
    futures::executor::block_on(async {
        let store = SqliteStore::open(db.path()).await.expect("open store");
        store
            .insert_sticker(detail(StickerType::Command, StickerState::Open, "cmd"))
            .await
            .expect("insert command");
        store
            .insert_sticker(detail(StickerType::Markdown, StickerState::Open, "# note"))
            .await
            .expect("insert markdown");
        store
            .insert_sticker(detail(StickerType::Timer, StickerState::Open, "{}"))
            .await
            .expect("insert timer");
        store
            .insert_sticker(detail(StickerType::Paint, StickerState::Open, "{}"))
            .await
            .expect("insert paint");
        store
            .insert_sticker(detail(StickerType::File, StickerState::Open, "{}"))
            .await
            .expect("insert file");

        let found = store.get_command_stickers().await.expect("query");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].sticker_type, StickerType::Command);
        assert_eq!(found[0].content, "cmd");
    });
}

#[test]
fn command_stickers_include_closed_and_populate_placements() {
    let db = TempDb::new("closed");
    futures::executor::block_on(async {
        let store = SqliteStore::open(db.path()).await.expect("open store");
        let open_id = store
            .insert_sticker(detail(
                StickerType::Command,
                StickerState::Open,
                r#"{"command":"ls"}"#,
            ))
            .await
            .expect("insert open");
        let closed_id = store
            .insert_sticker(detail(
                StickerType::Command,
                StickerState::Close,
                r#"{"command":"pwd"}"#,
            ))
            .await
            .expect("insert closed");

        store
            .upsert_sticker_placement(
                closed_id,
                StickerPlacement {
                    display_uuid: "a".into(),
                    display_id: Some(7),
                    native_left: 100,
                    native_top: 30,
                    native_width: 315,
                    native_height: 171,
                    scale_factor: 1.5,
                    updated_at: 1,
                },
                Some("a".into()),
            )
            .await
            .expect("upsert placement");

        let found = store.get_command_stickers().await.expect("query");
        let ids: Vec<i64> = found.iter().map(|s| s.id).collect();
        assert!(ids.contains(&open_id), "open command missing: {ids:?}");
        assert!(ids.contains(&closed_id), "closed command missing: {ids:?}");

        let closed = found.iter().find(|s| s.id == closed_id).expect("closed");
        assert_eq!(closed.state, StickerState::Close);
        assert_eq!(closed.placements.len(), 1);
        assert_eq!(closed.placements[0].display_uuid, "a");
        assert_eq!(closed.placements[0].native_left, 100);
    });
}

#[test]
fn command_stickers_preserve_malformed_and_raw_content() {
    let db = TempDb::new("malformed");
    futures::executor::block_on(async {
        let store = SqliteStore::open(db.path()).await.expect("open store");
        let cases = [
            "{not valid json",
            "echo hello && ls -la",
            r#"{"command":"ls",,}"#,
            "",
            r#"{"command":"ls","accept_selection":true}"#,
        ];
        for content in cases {
            store
                .insert_sticker(detail(StickerType::Command, StickerState::Open, content))
                .await
                .expect("insert");
        }

        let found = store.get_command_stickers().await.expect("query");
        let mut contents: Vec<&str> = found.iter().map(|s| s.content.as_str()).collect();
        contents.sort_unstable();
        let mut expected = cases.to_vec();
        expected.sort_unstable();
        assert_eq!(contents, expected);
    });
}

#[test]
fn command_stickers_order_updated_desc_then_id_desc() {
    let db = TempDb::new("order");
    futures::executor::block_on(async {
        let store = SqliteStore::open(db.path()).await.expect("open store");
        let oldest = store
            .insert_sticker(detail(StickerType::Command, StickerState::Open, "oldest"))
            .await
            .expect("insert oldest");
        let middle = store
            .insert_sticker(detail(StickerType::Command, StickerState::Open, "middle"))
            .await
            .expect("insert middle");
        let newest = store
            .insert_sticker(detail(StickerType::Command, StickerState::Open, "newest"))
            .await
            .expect("insert newest");

        // Pin timestamps explicitly: prove both ordering keys without timing sleeps.
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db.path().display()))
            .await
            .unwrap();
        for (id, timestamp) in [(oldest, 10), (middle, 20), (newest, 20)] {
            sqlx::query("UPDATE stickers SET updated_at = ? WHERE id = ?")
                .bind(timestamp)
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
        pool.close().await;

        let ids: Vec<i64> = store
            .get_command_stickers()
            .await
            .expect("query")
            .iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec![newest, middle, oldest]);

        // The projection is deterministic: a second read returns the same order.
        let again: Vec<i64> = store
            .get_command_stickers()
            .await
            .expect("query again")
            .iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(again, ids);
    });
}
