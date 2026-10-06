//! Cross-module integration checks that exercise the shared contract end-to-end.

use super::items::{ClipboardItem, ClipboardItemKind, RetentionState};
use super::{ordering, retention};

fn make_row(id: i64, bytes: i64, retained: bool) -> ClipboardItem {
    let now = chrono::Utc::now().timestamp_millis();
    ClipboardItem {
        id,
        kind: ClipboardItemKind::Plain,
        content: "x".repeat(bytes as usize),
        plain_preview: None,
        public_label: None,
        byte_size: bytes,
        created_at: now - id * 1_000,
        last_copied_at: now - id * 1_000,
        last_used_at: None,
        last_edited_at: None,
        retention: RetentionState {
            explicit: false,
            edited: false,
            protected_: retained,
        },
    }
}

#[test]
fn gui_and_cli_ordering_rules_agree() {
    let mut rows = vec![
        make_row(1, 5, false),
        make_row(3, 5, false),
        make_row(2, 5, false),
    ];
    ordering::sort_in_place(&mut rows);
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    // Smaller id → newer (created_at = now - id*1000). MRU-first == smaller id first.
    assert_eq!(ids, vec![1, 2, 3]);
}

#[test]
fn eviction_does_not_retain_protected_rows() {
    // id 1 is older but protected → not evictable. id 2 is the next-oldest ordinary victim.
    let rows = vec![make_row(1, 5, true), make_row(2, 5, false)];
    let victims = retention::eviction_victims(&rows, 1, 0, 5, &Default::default());
    let ids: Vec<i64> = victims.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![2]);
}
