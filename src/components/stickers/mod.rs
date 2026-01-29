use gpui::{AnyElement, App, Context, Entity, IntoElement, Render, Size};

use crate::model::sticker::StickerColor;

pub mod command;
pub mod markdown;
pub mod timer;

pub trait Sticker: Sized {
    // If return false, it means we should not close the sticker window.
    fn save_on_close(&mut self, cx: &mut Context<Self>) -> bool;

    fn min_window_size() -> Size<i32>;
    fn default_window_size() -> Size<i32>;

    fn set_color(&mut self, color: StickerColor);
}

pub trait StickerView {
    fn element(&self) -> AnyElement;
    fn save_on_close(&self, cx: &mut App) -> bool;
    fn set_color(&mut self, cx: &mut App, color: StickerColor);
}

pub struct StickerViewEntity<T: Render + Sticker + 'static> {
    entity: Entity<T>,
}

impl<T: Render + Sticker + 'static> StickerViewEntity<T> {
    pub fn new(entity: Entity<T>) -> Self {
        Self { entity }
    }
}

impl<T: Render + Sticker + 'static> StickerView for StickerViewEntity<T> {
    fn element(&self) -> AnyElement {
        self.entity.clone().into_any_element()
    }

    fn save_on_close(&self, cx: &mut App) -> bool {
        let mut is_success = false;
        let _ = self.entity.update(cx, |this, cx| {
            is_success = this.save_on_close(cx);
        });
        is_success
    }

    fn set_color(&mut self, cx: &mut App, color: StickerColor) {
        let _ = self.entity.update(cx, |this, _cx| {
            this.set_color(color);
        });
    }
}
