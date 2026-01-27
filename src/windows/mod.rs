use crate::model::sticker::StickerColor;

pub mod main;
pub mod sticker;

#[derive(Debug, Clone)]
pub enum StickerWindowEvent {
    Closed { id: i64 },
    ColorChanged { id: i64, color: StickerColor },
    TitleChanged { id: i64, title: String },
}
