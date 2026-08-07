use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
pub enum StickerType {
    Markdown,
    Timer,
    Command,
    Paint,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StickerOrderBy {
    CreatedAsc,
    CreatedDesc,
    UpdatedAsc,
    UpdatedDesc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
pub enum StickerState {
    #[serde(rename = "open")]
    Open,
    #[serde(rename = "close")]
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum StickerColor {
    Yellow,
    Green,
    Blue,
    Pink,
    Gray,
}

/// A command sticker that a cron expression has armed, as the background scheduler sees it.
///
/// Only what a headless run needs: no geometry, no placements, no colour. The scheduler never
/// opens a window, so loading those would be wasted work on every refresh.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScheduledCommand {
    pub id: i64,
    pub title: String,
    pub content: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StickerBrief {
    pub id: i64,
    pub title: String,
    pub state: StickerState,
    pub color: StickerColor,
    #[sqlx(rename = "type")]
    pub sticker_type: StickerType,
    pub created_at: i64,
    pub updated_at: i64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StickerDetail {
    pub id: i64,
    pub title: String,
    pub state: StickerState,
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub top_most: bool,
    pub color: StickerColor,
    #[sqlx(rename = "type")]
    pub sticker_type: StickerType,
    pub content: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// The display this sticker was last on (as the raw platform display ID).
    pub display_id: Option<i64>,
    /// Stable platform identifier for the physical monitor.
    pub display_uuid: Option<String>,
    /// Windows virtual desktop GUID. `None` on other platforms.
    pub virtual_desktop_id: Option<String>,
    /// Native Windows virtual-screen X coordinate, unaffected by monitor DPI.
    pub native_left: Option<i32>,
    /// Native Windows virtual-screen Y coordinate, unaffected by monitor DPI.
    pub native_top: Option<i32>,
    /// Native Windows window width in physical pixels.
    pub native_width: Option<i32>,
    /// Native Windows window height in physical pixels.
    pub native_height: Option<i32>,
    /// The monitor the user last deliberately moved this sticker to. The sticker returns here
    /// whenever that monitor is connected, even if it had to live elsewhere in the meantime.
    pub preferred_display_uuid: Option<String>,
    /// One remembered placement per monitor. Loaded alongside the sticker, never stored inline.
    #[sqlx(skip)]
    pub placements: Vec<StickerPlacement>,
}

/// A placement a sticker had on one specific monitor, in native (physical) pixels.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct StickerPlacement {
    /// Stable platform identifier of the monitor this placement belongs to.
    pub display_uuid: String,
    /// Raw platform display handle at capture time. Diagnostics only, it is not stable.
    pub display_id: Option<i64>,
    pub native_left: i32,
    pub native_top: i32,
    pub native_width: i32,
    pub native_height: i32,
    /// The monitor's DPI scale when the rect was captured.
    pub scale_factor: f32,
    /// Unix milliseconds of the last user-initiated move onto this monitor. Drives both the
    /// least-recently-used eviction and the "which monitor was most recently chosen" question.
    pub updated_at: i64,
}

/// The maximum number of monitors a single sticker remembers a placement for.
pub const MAX_PLACEMENTS_PER_STICKER: usize = 4;

/// Decide which placement records to drop so a sticker never remembers more than `max` monitors.
///
/// The record for `protect_uuid` (the current primary monitor) is never dropped, so a sticker
/// always has a fallback to come home to.
pub fn prune_placements(
    placements: &[StickerPlacement],
    protect_uuid: Option<&str>,
    max: usize,
) -> Vec<String> {
    let is_protected =
        |placement: &StickerPlacement| Some(placement.display_uuid.as_str()) == protect_uuid;

    let protected_count = placements.iter().filter(|p| is_protected(p)).count();
    let mut evictable: Vec<&StickerPlacement> =
        placements.iter().filter(|p| !is_protected(p)).collect();
    evictable.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.display_uuid.cmp(&b.display_uuid))
    });

    evictable
        .into_iter()
        .skip(max.saturating_sub(protected_count))
        .map(|placement| placement.display_uuid.clone())
        .collect()
}

/// Everything that is persisted about where a sticker window sits.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StickerBounds {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub display_id: Option<i64>,
    pub display_uuid: Option<String>,
    pub virtual_desktop_id: Option<String>,
    pub native_left: Option<i32>,
    pub native_top: Option<i32>,
    pub native_width: Option<i32>,
    pub native_height: Option<i32>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StickerListItem {
    pub id: i64,
    pub title: String,
    pub state: StickerState,
    #[sqlx(rename = "type")]
    pub sticker_type: StickerType,
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl StickerColor {
    pub const ALL: [Self; 5] = [
        Self::Pink,
        Self::Yellow,
        Self::Green,
        Self::Blue,
        Self::Gray,
    ];

    #[cfg(feature = "ui")]
    pub fn bg(&self) -> gpui::Rgba {
        use gpui::rgb;

        match self {
            Self::Yellow => rgb(0x2d2a1b),
            Self::Green => rgb(0x1b2d20),
            Self::Blue => rgb(0x1b2430),
            Self::Pink => rgb(0x2d1b24),
            Self::Gray => rgb(0x1e1e1e),
        }
    }

    #[cfg(feature = "ui")]
    pub fn swatch(&self) -> gpui::Rgba {
        use gpui::rgb;

        match self {
            Self::Yellow => rgb(0xf2c94c),
            Self::Green => rgb(0x27ae60),
            Self::Blue => rgb(0x2d9cdb),
            Self::Pink => rgb(0xeb5757),
            Self::Gray => rgb(0xbdbdbd),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Yellow => "yellow",
            Self::Green => "green",
            Self::Blue => "blue",
            Self::Pink => "pink",
            Self::Gray => "gray",
        }
    }
}

impl FromStr for StickerColor {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "yellow" => Ok(Self::Yellow),
            "green" => Ok(Self::Green),
            "blue" => Ok(Self::Blue),
            "pink" => Ok(Self::Pink),
            _ => Ok(Self::Gray), // Default fallback
        }
    }
}

#[cfg(test)]
mod placement_tests {
    use super::*;

    fn placement(uuid: &str, updated_at: i64) -> StickerPlacement {
        StickerPlacement {
            display_uuid: uuid.into(),
            display_id: None,
            native_left: 0,
            native_top: 0,
            native_width: 210,
            native_height: 114,
            scale_factor: 1.0,
            updated_at,
        }
    }

    #[test]
    fn pruning_keeps_the_most_recently_used_monitors() {
        let placements = [
            placement("a", 1),
            placement("b", 2),
            placement("c", 3),
            placement("d", 4),
            placement("e", 5),
        ];
        assert_eq!(
            prune_placements(&placements, None, 4),
            vec!["a".to_string()]
        );
    }

    #[test]
    fn pruning_never_evicts_the_primary_even_when_it_is_the_oldest() {
        let placements = [
            placement("primary", 1),
            placement("b", 2),
            placement("c", 3),
            placement("d", 4),
            placement("e", 5),
        ];
        assert_eq!(
            prune_placements(&placements, Some("primary"), 4),
            vec!["b".to_string()]
        );
    }

    #[test]
    fn pruning_is_a_no_op_below_the_cap() {
        let placements = [placement("a", 1), placement("b", 2)];
        assert!(prune_placements(&placements, Some("a"), 4).is_empty());
    }
}
