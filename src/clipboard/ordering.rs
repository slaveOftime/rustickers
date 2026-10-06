//! Shared ordering and tie-breaking.
//!
//! Both GUI and CLI use the same comparator so a row's position is identical across surfaces.

use super::items::{ClipboardItem, RetentionState};

/// Stable comparator for the chooser / CLI list.
///
/// Rules, in order:
/// 1. Larger `recency_at_ms` first.
/// 2. Larger `created_at` to break ties deterministically (older vs newer rows that share a
///    timestamp).
/// 3. Larger `id` to break rare millisecond collisions deterministically.
///
/// Comparisons also group retained rows above pure-MRU by recency: once we have decided two
/// items belong together, retention does not change their relative order, but it keeps them out of
/// the ordinary-eviction pool.
pub fn compare_by_recency(a: &ClipboardItem, b: &ClipboardItem) -> std::cmp::Ordering {
    let ra = super::items::recency_at_ms(a);
    let rb = super::items::recency_at_ms(b);
    rb.cmp(&ra)
        .then_with(|| b.created_at.cmp(&a.created_at))
        .then_with(|| b.id.cmp(&a.id))
}

/// Stable in-place sort of `items` according to [`compare_by_recency`].
pub fn sort_in_place(items: &mut [ClipboardItem]) {
    items.sort_by(compare_by_recency);
}

/// The recency key used to evict least-recent ordinary items. We use the same key as the chooser
/// so the row at the bottom is the row that gets evicted.
pub fn recency_key(item: &ClipboardItem) -> i64 {
    super::items::recency_at_ms(item)
}

/// Helper for backends that want to mark items as "ordinary" or "retained" without re-creating the
/// struct.
pub const fn empty_retention() -> RetentionState {
    RetentionState {
        explicit: false,
        edited: false,
        protected_: false,
    }
}
