use std::str::FromStr;

use gpui::rgb;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
pub enum StickerType {
    Markdown,
    Timer,
    Command,
    Paint,
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

#[derive(Debug, Clone, Copy, PartialEq, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
pub enum StickerColor {
    Yellow,
    Green,
    Blue,
    Pink,
    Gray,
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
}

impl StickerColor {
    pub const ALL: [Self; 5] = [
        Self::Pink,
        Self::Yellow,
        Self::Green,
        Self::Blue,
        Self::Gray,
    ];

    pub fn bg(&self) -> gpui::Rgba {
        match self {
            Self::Yellow => rgb(0x2d2a1b),
            Self::Green => rgb(0x1b2d20),
            Self::Blue => rgb(0x1b2430),
            Self::Pink => rgb(0x2d1b24),
            Self::Gray => rgb(0x1e1e1e),
        }
    }

    pub fn swatch(&self) -> gpui::Rgba {
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
