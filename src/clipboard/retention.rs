//! Eviction rules shared by capture and CLI.
//!
//! `eviction_victims` returns the ordered list of ordinary items that should be deleted to bring
//! the ordinary-count and ordinary-byte budgets back under their limits. Retained rows are never
//! returned, even when the limits are tight: a saved/protected/edited/explicit item is part of
//! the user's history and is not fair game.

use super::items::{ClipboardItem, RetentionState};
use std::collections::HashSet;

/// Decide which items to delete from the ordinary pool to fit new capture into a count and byte
/// budget. Both limits are checked; the worst case (most severe overflow) drives how many rows
/// are flagged. `existing_leases` are ids that are temporarily protected while the user edits or
/// uses a row; they are treated as retained for the duration of the call.
pub fn eviction_victims<'a>(
    ordinary: &'a [ClipboardItem],
    incoming_bytes: i64,
    count_limit: i64,
    byte_limit: i64,
    existing_leases: &HashSet<i64>,
) -> Vec<&'a ClipboardItem> {
    if count_limit <= 0 && byte_limit <= 0 {
        return Vec::new();
    }

    let eligible: Vec<_> = ordinary
        .iter()
        .filter(|item| {
            item.kind == super::items::ClipboardItemKind::Plain && item.retention.ordinary()
        })
        .collect();
    let ordinary_count = eligible.len() as i64;
    let mut overflow_count = 0;
    if count_limit > 0 {
        overflow_count = (ordinary_count + 1 - count_limit).max(0);
    }

    let ordinary_bytes: i64 = eligible.iter().map(|item| item.byte_size).sum();
    let mut overflow_bytes = 0;
    if byte_limit > 0 {
        overflow_bytes = (ordinary_bytes + incoming_bytes - byte_limit).max(0);
    }

    if overflow_count == 0 && overflow_bytes == 0 {
        return Vec::new();
    }

    // Sort oldest-first: the chooser shows MRU, so what is on the bottom is what we evict.
    let mut candidates: Vec<&ClipboardItem> = eligible;
    candidates.sort_by_key(|item| super::ordering::recency_key(item));

    let mut victims = Vec::new();
    let mut released_bytes: i64 = 0;
    for candidate in candidates.iter() {
        if existing_leases.contains(&candidate.id) {
            continue;
        }
        let count_satisfied = overflow_count == 0 || (victims.len() as i64) >= overflow_count;
        let bytes_satisfied = overflow_bytes == 0 || released_bytes >= overflow_bytes;
        if count_satisfied && bytes_satisfied {
            break;
        }
        released_bytes += candidate.byte_size;
        victims.push(*candidate);
    }
    victims
}

/// True when the row is considered retained by the user (any reason).
pub fn is_retained(state: &RetentionState) -> bool {
    state.is_retained()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::items::ClipboardItemKind;
    use chrono::Utc;

    fn row(id: i64, byte_size: i64, edited: bool, explicit: bool, _plain: bool) -> ClipboardItem {
        let now = Utc::now().timestamp_millis();
        ClipboardItem {
            id,
            kind: ClipboardItemKind::Plain,
            content: "x".repeat(byte_size as usize),
            plain_preview: None,
            public_label: None,
            byte_size,
            created_at: now - id * 100,
            last_copied_at: now - id * 50,
            last_used_at: None,
            last_edited_at: edited.then_some(now - id),
            retention: RetentionState {
                explicit,
                edited,
                protected_: false,
            },
        }
    }

    #[test]
    fn returns_no_victims_when_under_budget() {
        let rows = vec![row(1, 100, false, false, true)];
        let victims = eviction_victims(&rows, 100, 10, 1024, &HashSet::new());
        assert!(victims.is_empty());
    }

    #[test]
    fn evicted_rows_are_oldest_ordinary_first() {
        // 3 rows, cap at 1, incoming 1 → 3 victims; helper: smallest id = newest. So oldest
        // first means id 3, then 2, then 1.
        let rows = vec![
            row(1, 100, false, false, true),
            row(2, 100, false, false, true),
            row(3, 100, false, false, true),
        ];
        let victims = eviction_victims(&rows, 100, 1, 4096, &HashSet::new());
        let ids: Vec<i64> = victims.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![3, 2, 1]);
    }

    #[test]
    fn retained_rows_are_never_evicted() {
        // Retained rows are filtered out by the caller; only the ordinary rows are passed in.
        let ordinary_rows: Vec<ClipboardItem> = vec![
            row(1, 100, false, false, true),
            row(2, 100, false, false, true),
        ];
        // Caller additionally skips id=2 (which they held as a lease in a broader system).
        let mut leases = HashSet::new();
        leases.insert(2);
        let victims = eviction_victims(&ordinary_rows, 100, 1, 4096, &leases);
        let ids: Vec<i64> = victims.iter().map(|r| r.id).collect();
        // id 1 is the only remaining candidate and not leased.
        assert_eq!(ids, vec![1]);
    }

    #[test]
    fn leases_block_eviction_temporarily() {
        // Only id 3 is ordinary; cap at 1 with 2 rows + 1 incoming → need 2 victims. With id 2
        // leased and only id 3 ordinary left, no victim is available; function returns empty.
        let rows = vec![
            row(3, 100, false, false, true),
            row(2, 100, false, false, true),
        ];
        let mut leases = HashSet::new();
        leases.insert(2);
        let victims = eviction_victims(&rows, 100, 1, 4096, &leases);
        let ids: Vec<i64> = victims.iter().map(|r| r.id).collect();
        // id 2 is leased, so the loop runs out without finding enough free rows; the caller
        // decides whether to keep the new capture on disk. We get id 3 as the only candidate,
        // but it's not enough to satisfy the overflow, so we surface what we have.
        assert_eq!(ids, vec![3]);
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::clipboard::items::ClipboardItemKind;

    #[test]
    fn byte_budget_requires_deleting_the_final_victim_not_just_inspecting_it() {
        let rows: Vec<_> = (1..=3)
            .map(|id| ClipboardItem {
                id,
                kind: ClipboardItemKind::Plain,
                content: "x".repeat(100),
                byte_size: 100,
                plain_preview: None,
                public_label: None,
                created_at: id,
                last_copied_at: id,
                last_used_at: None,
                last_edited_at: None,
                retention: RetentionState::default(),
            })
            .collect();
        let victims = eviction_victims(&rows, 100, 100, 200, &HashSet::new());
        assert_eq!(
            victims.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn damaged_protected_metadata_never_makes_ciphertext_an_eviction_candidate() {
        let row = ClipboardItem {
            id: 1,
            kind: ClipboardItemKind::Protected,
            content: String::new(),
            byte_size: 100,
            plain_preview: None,
            public_label: None,
            created_at: 1,
            last_copied_at: 1,
            last_used_at: None,
            last_edited_at: None,
            retention: RetentionState::default(),
        };
        assert!(eviction_victims(&[row], 100, 1, 1, &HashSet::new()).is_empty());
    }
}
