use rustickers::model::sticker::*;
use rustickers::storage::StickerStore;
use rustickers::storage::sqlite::SqliteStore;

fn detail() -> StickerDetail {
    StickerDetail {
        id: 0,
        title: "t".into(),
        state: StickerState::Open,
        left: 10,
        top: 20,
        width: 210,
        height: 114,
        top_most: false,
        color: StickerColor::Yellow,
        sticker_type: StickerType::Markdown,
        content: "".into(),
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

fn placement(uuid: &str, left: i32, updated_at: i64) -> StickerPlacement {
    StickerPlacement {
        display_uuid: uuid.into(),
        display_id: Some(7),
        native_left: left,
        native_top: 30,
        native_width: 315,
        native_height: 171,
        scale_factor: 1.5,
        updated_at,
    }
}

#[test]
fn placements_survive_migration_and_pruning() {
    let dir = std::env::temp_dir().join(format!("rustickers-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("stickers.db");

    futures::executor::block_on(async {
        let store = SqliteStore::open(&db).await.expect("open store");
        let id = store.insert_sticker(detail()).await.expect("insert");

        store
            .update_sticker_bounds(
                id,
                StickerBounds {
                    left: 1,
                    top: 2,
                    width: 210,
                    height: 114,
                    display_id: Some(7),
                    display_uuid: Some("a".into()),
                    virtual_desktop_id: Some("vd".into()),
                    native_left: Some(100),
                    native_top: Some(30),
                    native_width: Some(315),
                    native_height: Some(171),
                },
            )
            .await
            .expect("bounds");

        for (index, uuid) in ["a", "b", "c", "d", "e"].iter().enumerate() {
            store
                .upsert_sticker_placement(
                    id,
                    placement(uuid, index as i32 * 10, index as i64),
                    Some("a".into()),
                )
                .await
                .expect("upsert placement");
        }

        store
            .update_sticker_preferred_display(id, Some("c".into()))
            .await
            .expect("preferred");

        let loaded = store.get_sticker(id).await.expect("get");
        assert_eq!(loaded.preferred_display_uuid.as_deref(), Some("c"));

        let uuids: Vec<&str> = loaded
            .placements
            .iter()
            .map(|p| p.display_uuid.as_str())
            .collect();
        // "a" is protected, "b" is the least recently used of the rest.
        assert_eq!(uuids.len(), MAX_PLACEMENTS_PER_STICKER);
        assert!(uuids.contains(&"a"));
        assert!(!uuids.contains(&"b"));

        let round_tripped = loaded
            .placements
            .iter()
            .find(|p| p.display_uuid == "e")
            .expect("newest placement");
        assert_eq!(round_tripped.native_left, 40);
        assert_eq!(round_tripped.scale_factor, 1.5);

        store.delete_sticker(id).await.expect("delete");
        assert!(store.get_sticker_placements(id).await.unwrap().is_empty());
    });

    let _ = std::fs::remove_dir_all(&dir);
}
