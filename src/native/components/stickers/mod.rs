use gpui::{AnyElement, App, Context, Entity, IntoElement, Render, Size, Window};

use crate::model::sticker::StickerColor;

pub mod command;
mod content_lock;
pub mod file;
pub mod markdown;
pub mod paint;
pub mod timer;

pub(crate) use content_lock::FOCUS_LOSS_RELOCK_DELAY;

pub trait Sticker: Sized {
    fn id(&self) -> i64;

    // If return false, it means we should not close the sticker window.
    fn save_on_close(&mut self, cx: &mut Context<Self>) -> bool;

    fn min_window_size() -> Size<i32>;
    fn default_window_size() -> Size<i32>;

    fn set_color(&mut self, color: StickerColor);

    fn use_default_bg(&self) -> bool {
        true
    }

    fn disable_color_picker(&self) -> bool {
        false
    }

    fn suppress_window_escape(&self) -> bool {
        false
    }

    fn protected_content_visible(&self) -> bool {
        false
    }

    fn relock_protected_content(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn handle_lock_shortcut(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        false
    }

    fn header_extension(&mut self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        None
    }

    fn footer_extension(&mut self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        None
    }

    fn is_footer_absoute(&self) -> bool {
        true
    }
}

pub trait StickerView {
    fn id(&self, cx: &App) -> i64;
    fn element(&self) -> AnyElement;
    fn save_on_close(&self, cx: &mut App) -> bool;
    fn set_color(&mut self, cx: &mut App, color: StickerColor);
    fn use_default_bg(&self, cx: &App) -> bool;
    fn disable_color_picker(&self, cx: &App) -> bool;
    fn suppress_window_escape(&self, cx: &App) -> bool;
    fn protected_content_visible(&self, cx: &App) -> bool;
    fn relock_protected_content(&self, window: &mut Window, cx: &mut App);
    fn handle_lock_shortcut(&self, window: &mut Window, cx: &mut App) -> bool;
    fn header_extension(&self, cx: &mut App) -> Option<AnyElement>;
    fn footer_extension(&self, cx: &mut App) -> Option<AnyElement>;
    fn is_footer_absoute(&self, cx: &App) -> bool;
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
    fn id(&self, cx: &App) -> i64 {
        self.entity.read(cx).id()
    }

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

    fn use_default_bg(&self, cx: &App) -> bool {
        self.entity.read(cx).use_default_bg()
    }

    fn disable_color_picker(&self, cx: &App) -> bool {
        self.entity.read(cx).disable_color_picker()
    }

    fn suppress_window_escape(&self, cx: &App) -> bool {
        self.entity.read(cx).suppress_window_escape()
    }

    fn protected_content_visible(&self, cx: &App) -> bool {
        self.entity.read(cx).protected_content_visible()
    }

    fn relock_protected_content(&self, window: &mut Window, cx: &mut App) {
        let _ = self.entity.update(cx, |this, cx| {
            this.relock_protected_content(window, cx);
        });
    }

    fn handle_lock_shortcut(&self, window: &mut Window, cx: &mut App) -> bool {
        self.entity
            .update(cx, |this, cx| this.handle_lock_shortcut(window, cx))
    }

    fn header_extension(&self, cx: &mut App) -> Option<AnyElement> {
        self.entity.update(cx, |this, cx| this.header_extension(cx))
    }

    fn footer_extension(&self, cx: &mut App) -> Option<AnyElement> {
        self.entity.update(cx, |this, cx| this.footer_extension(cx))
    }

    fn is_footer_absoute(&self, cx: &App) -> bool {
        self.entity.read(cx).is_footer_absoute()
    }
}
