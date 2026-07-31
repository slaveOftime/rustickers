use crate::model::sticker::StickerColor;

pub mod main;
pub mod selection;
pub mod sticker;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum StickerWindowEvent {
    Closed { id: i64 },
    Created { id: i64 },
    ColorChanged { id: i64, color: StickerColor },
    TitleChanged { id: i64, title: String },
}
