//! UI-owned palette for [`crate::model::sticker::StickerColor`].
//!
//! The model layer keeps the [`StickerColor`] enum as a serialised
//! identity (used in storage, IPC, and CLI). The presentation of that
//! identity — the surface colour of a sticker card, the swatch shown in
//! the colour picker, the swatch dot on a paint stroke — lives here so
//! the model never embeds raw GPUI presentation literals.
//!
//! Override the whole palette at startup if you need to bring the
//! sticker surface colours into line with a custom theme; the model will
//! continue to flow through this module without modification.

use crate::model::sticker::StickerColor;
pub use gpui::Rgba;

/// Shared alpha for desktop window surfaces; text and controls remain fully opaque.
pub(crate) const WINDOW_SURFACE_OPACITY: f32 = 0.95;

/// Background tint applied to a sticker card's surface. Dark, slightly
/// desaturated so the sticker's title and body stay legible while the
/// domain colour still reads.
pub fn sticker_background(color: StickerColor) -> Rgba {
    use gpui::rgb;

    match color {
        StickerColor::Yellow => rgb(0x2d2a1b),
        StickerColor::Green => rgb(0x1b2d20),
        StickerColor::Blue => rgb(0x1b2430),
        StickerColor::Pink => rgb(0x2d1b24),
        StickerColor::Gray => rgb(0x1e1e1e),
    }
}

/// Bright swatch colour used as the colour-picker dot and the
/// `StickerColor::swatch()` accessor on the model layer.
pub fn sticker_swatch(color: StickerColor) -> Rgba {
    use gpui::rgb;

    match color {
        StickerColor::Yellow => rgb(0xf2c94c),
        StickerColor::Green => rgb(0x27ae60),
        StickerColor::Blue => rgb(0x2d9cdb),
        StickerColor::Pink => rgb(0xeb5757),
        StickerColor::Gray => rgb(0xbdbdbd),
    }
}
