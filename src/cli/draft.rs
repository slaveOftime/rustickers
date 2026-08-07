//! The one path through which the CLI creates a sticker.
//!
//! Every create command (`markdown`, `cmd`, and the skills built on them) fills in a
//! [`StickerDraft`] and hands it here, so placement defaults, the top-most follow-up write and the
//! "tell the running app about it" step behave identically no matter which command you used.

use crate::model::sticker::{StickerColor, StickerDetail, StickerState, StickerType};
use crate::storage::ArcStickerStore;

use super::output::Format;
use super::runtime::{self, Delivery, block_on};

/// Where a sticker lands and how it looks. Shared by every create command.
#[derive(clap::Args, Debug, Clone)]
pub struct Geometry {
    /// Sticker width in pixels
    #[arg(long, value_name = "PX", help_heading = "Appearance")]
    pub width: Option<i32>,

    /// Sticker height in pixels
    #[arg(long, value_name = "PX", help_heading = "Appearance")]
    pub height: Option<i32>,

    /// Distance from the left edge of the screen, in pixels
    #[arg(long, value_name = "PX", help_heading = "Appearance")]
    pub left: Option<i32>,

    /// Distance from the top edge of the screen, in pixels
    #[arg(long, value_name = "PX", help_heading = "Appearance")]
    pub top: Option<i32>,

    /// Sticker color: yellow, green, blue, pink or gray
    #[arg(long, value_parser = super::parse_color, help_heading = "Appearance")]
    pub color: Option<StickerColor>,

    /// Keep the sticker above other windows
    #[arg(long, help_heading = "Appearance")]
    pub top_most: bool,

    /// Create the sticker without opening a window
    ///
    /// The sticker still exists and is still eligible for selection runs; it just does not
    /// appear until you `open` it.
    #[arg(long, help_heading = "Appearance")]
    pub closed: bool,
}

impl Geometry {
    fn state(&self) -> StickerState {
        if self.closed {
            StickerState::Close
        } else {
            StickerState::Open
        }
    }
}

/// A sticker that does not exist yet.
pub struct StickerDraft {
    pub title: String,
    pub sticker_type: StickerType,
    pub content: String,
    pub default_color: StickerColor,
    pub default_width: i32,
    pub default_height: i32,
}

/// The outcome of creating a sticker, and whether the desktop app heard about it.
pub struct Created {
    pub id: i64,
    pub state: StickerState,
    pub delivery: Delivery,
}

impl StickerDraft {
    pub fn create(self, store: &ArcStickerStore, geometry: &Geometry) -> anyhow::Result<Created> {
        let state = geometry.state();

        let id = block_on(store.insert_sticker(StickerDetail {
            id: 0,
            title: self.title,
            state,
            left: geometry.left.unwrap_or(100),
            top: geometry.top.unwrap_or(100),
            width: geometry.width.unwrap_or(self.default_width),
            height: geometry.height.unwrap_or(self.default_height),
            top_most: geometry.top_most,
            color: geometry.color.unwrap_or(self.default_color),
            sticker_type: self.sticker_type,
            content: self.content,
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
        }))?;

        // `insert_sticker` only writes the columns it knows a new sticker needs, so top-most is a
        // separate update rather than part of the row above.
        if geometry.top_most {
            block_on(store.update_sticker_top_most(id, true))?;
        }

        let delivery = if state == StickerState::Open {
            runtime::open_sticker(id)
        } else {
            Delivery::AppNotRunning
        };

        Ok(Created {
            id,
            state,
            delivery,
        })
    }
}

impl Created {
    /// The human-facing summary shared by every create command.
    pub fn report(&self, format: Format, kind: &str) {
        format.note(format!("Created {kind} sticker (id={})", self.id));
        match (self.state, self.delivery) {
            (StickerState::Close, _) => format.note(format!(
                "It was created closed. Run `rusticker open {}` to show it.",
                self.id
            )),
            (StickerState::Open, Delivery::Delivered) => {
                format.note("Rustickers is running and opened it.");
            }
            (StickerState::Open, Delivery::AppNotRunning) => {
                format.note("Rustickers is not running — it will open on next launch.");
            }
        }
    }

    pub fn json(&self, kind: &str) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "type": kind,
            "state": match self.state {
                StickerState::Open => "open",
                StickerState::Close => "close",
            },
            "opened": self.delivery.delivered(),
            "app_running": self.delivery.delivered(),
        })
    }
}
