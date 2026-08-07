use crate::model::sticker::StickerColor;
use std::sync::RwLock;

pub mod main;
pub mod placement;
pub mod selection;
pub mod sticker;
mod transient_topmost;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EscapeDismissTarget {
    Selection,
    Sticker(i64),
}

static ACTIVE_ESCAPE_DISMISS_TARGET: RwLock<Option<EscapeDismissTarget>> = RwLock::new(None);

pub(crate) fn set_escape_dismiss_target_active(target: EscapeDismissTarget, active: bool) {
    if let Ok(mut current) = ACTIVE_ESCAPE_DISMISS_TARGET.write() {
        if active {
            *current = Some(target);
        } else if *current == Some(target) {
            *current = None;
        }
    }
}

pub fn has_escape_dismiss_target() -> bool {
    ACTIVE_ESCAPE_DISMISS_TARGET
        .read()
        .is_ok_and(|target| target.is_some())
}

pub fn close_active_escape_target(cx: &mut gpui::App) -> bool {
    let target = ACTIVE_ESCAPE_DISMISS_TARGET
        .read()
        .ok()
        .and_then(|target| *target);
    match target {
        Some(EscapeDismissTarget::Selection) => selection::dispatch_escape(cx),
        Some(EscapeDismissTarget::Sticker(open_id)) => {
            sticker::StickerWindow::dispatch_escape(open_id, cx)
        }
        None => false,
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum StickerWindowEvent {
    Closed { id: i64 },
    Created { id: i64 },
    ColorChanged { id: i64, color: StickerColor },
    TitleChanged { id: i64, title: String },
}
