use std::sync::{Arc, RwLock, mpsc};

use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, Bounds, Context, Entity, IntoElement, Render,
    ScrollHandle, Subscription, Window, WindowBackgroundAppearance, WindowBounds,
    WindowControlArea, WindowKind, WindowOptions, div, prelude::*, px, rgba, size,
    transparent_black,
};
use gpui_component::{
    ActiveTheme, Icon, Root,
    button::Button,
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement,
    v_flex,
};

#[cfg(target_os = "macos")]
use cocoa::{
    appkit::NSApplication,
    base::{YES, nil},
};

use crate::{
    model::{content::CommandContent, sticker::StickerDetail},
    native::{components::IconName, windows::StickerWindowEvent},
    storage::ArcStickerStore,
};

use super::sticker::StickerWindow;

const POPUP_WIDTH: f32 = 300.0;
const POPUP_HEIGHT: f32 = 400.0;

static SELECTION_POPUP: RwLock<Option<AnyWindowHandle>> = RwLock::new(None);

pub struct SelectionPopup {
    store: ArcStickerStore,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    stickers: Arc<Vec<StickerDetail>>,
    filtered: Vec<usize>,
    selected: usize,
    selection: Arc<str>,
    query: Entity<InputState>,
    scroll_handle: ScrollHandle,
    closing: bool,
    _keystroke_subscription: Subscription,
}

impl SelectionPopup {
    pub fn open(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        stickers: Vec<StickerDetail>,
        selection: String,
    ) -> anyhow::Result<()> {
        let existing = SELECTION_POPUP
            .write()
            .ok()
            .and_then(|mut popup| popup.take());
        if let Some(existing) = existing {
            let _ = existing.update(cx, |_, window, _| window.remove_window());
            cx.defer(move |cx| {
                if let Err(err) = Self::open(cx, sticker_events_tx, store, stickers, selection) {
                    tracing::warn!(error = ?err, "Failed to replace selection popup");
                }
            });
            return Ok(());
        }

        let bounds = Bounds::centered(None, size(px(POPUP_WIDTH), px(POPUP_HEIGHT)), cx);
        let handle = cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(POPUP_WIDTH), px(POPUP_HEIGHT))),
                window_background: WindowBackgroundAppearance::Transparent,
                is_resizable: false,
                kind: WindowKind::Floating,
                titlebar: None,
                ..Default::default()
            },
            |window, cx| {
                let entity = cx
                    .new(|cx| Self::new(window, cx, sticker_events_tx, store, stickers, selection));
                cx.new(|cx| Root::new(entity, window, cx).bg(transparent_black().alpha(0.0)))
            },
        )?;

        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, _| {
                #[cfg(target_os = "macos")]
                unsafe {
                    NSApplication::sharedApplication(nil).activateIgnoringOtherApps_(YES);
                }
                window.refresh();
                window.activate_window();
            });
        });

        if let Ok(mut popup) = SELECTION_POPUP.write() {
            *popup = Some(handle.into());
        }

        Ok(())
    }

    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        stickers: Vec<StickerDetail>,
        selection: String,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Filter stickers ..."));
        let filtered = (0..stickers.len()).collect();

        cx.subscribe_in(
            &query,
            window,
            |this, query, event, _window, cx| match event {
                InputEvent::Change => {
                    let value = query.read(cx).value().to_string();
                    this.apply_filter(&value, cx);
                }
                InputEvent::PressEnter { .. } => this.open_selected(cx),
                _ => {}
            },
        )
        .detach();

        let entity = cx.weak_entity();
        let keystroke_subscription = cx.intercept_keystrokes(move |event, window, cx| {
            if !window.is_window_active() {
                return;
            }
            let Some(entity) = entity.upgrade() else {
                return;
            };
            let handled = match event.keystroke.key.as_str() {
                "up" => {
                    entity.update(cx, |this, cx| this.move_selection(-1, cx));
                    true
                }
                "down" => {
                    entity.update(cx, |this, cx| this.move_selection(1, cx));
                    true
                }
                "escape" => {
                    entity.update(cx, |this, cx| this.dismiss(cx));
                    true
                }
                _ => false,
            };
            if handled {
                cx.stop_propagation();
                window.prevent_default();
            }
        });

        window.on_window_should_close(cx, |_, _| {
            clear_popup_handle();
            true
        });

        query.update(cx, |query, cx| query.focus(window, cx));

        Self {
            store,
            sticker_events_tx,
            stickers: Arc::new(stickers),
            filtered,
            selected: 0,
            selection: Arc::from(selection),
            query,
            scroll_handle: ScrollHandle::new(),
            closing: false,
            _keystroke_subscription: keystroke_subscription,
        }
    }

    fn apply_filter(&mut self, query: &str, cx: &mut Context<Self>) {
        self.filtered = filtered_sticker_indices(&self.stickers, query);
        self.selected = 0;
        self.scroll_handle.scroll_to_item(0);
        cx.notify();
    }

    fn move_selection(&mut self, offset: isize, cx: &mut Context<Self>) {
        if self.filtered.is_empty() {
            return;
        }
        self.selected = (self.selected as isize + offset)
            .clamp(0, self.filtered.len().saturating_sub(1) as isize)
            as usize;
        self.scroll_handle.scroll_to_item(self.selected);
        cx.notify();
    }

    fn open_at(&mut self, filtered_index: usize, cx: &mut Context<Self>) {
        self.selected = filtered_index;
        self.open_selected(cx);
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let Some(&sticker_index) = self.filtered.get(self.selected) else {
            return;
        };
        self.closing = true;
        let detail = self.stickers[sticker_index].clone();
        let sticker_id = detail.id;
        let store = self.store.clone();
        let store_for_lru = store.clone();
        let sticker_events_tx = self.sticker_events_tx.clone();
        let selection = self.selection.to_string();

        tracing::info!(
            sticker_id,
            sticker_title = %detail.title,
            selection_len = selection.len(),
            "Selection command confirmed"
        );

        cx.spawn(async move |_, _| {
            if let Err(err) = store_for_lru
                .touch_selection_lru(sticker_id, crate::utils::time::now_unix_millis())
                .await
            {
                tracing::warn!(sticker_id, error = ?err, "Failed to update selection command LRU");
            }
        })
        .detach();

        cx.defer(close_popup);
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(50))
                .await;
            let result = cx.update(|cx| {
                tracing::debug!(sticker_id, "Opening confirmed selection command sticker");
                StickerWindow::open_with_selection(
                    cx,
                    sticker_events_tx,
                    store,
                    detail,
                    selection,
                )
            });
            if let Err(err) = result {
                tracing::warn!(sticker_id, error = ?err, "Failed to open selection command sticker");
            }
        })
        .detach();
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        tracing::debug!("Dismissing selection popup");
        cx.defer(close_popup);
    }

    fn row(&self, filtered_index: usize, cx: &mut Context<Self>) -> AnyElement {
        let sticker = &self.stickers[self.filtered[filtered_index]];
        let selected = filtered_index == self.selected;
        let title = sticker_label(sticker);
        let command = sticker_command(sticker);

        v_flex()
            .id(("selection-sticker", sticker.id as u64))
            .w_full()
            .px_3()
            .py_2()
            .cursor_pointer()
            .when(selected, |row| row.bg(rgba(0x3b82f655)))
            .hover(|row| row.bg(rgba(0xffffff12)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_at(filtered_index, cx);
            }))
            .child(div().child(title))
            .when(!command.is_empty(), |row| {
                row.child(
                    div()
                        .text_sm()
                        .opacity(0.7)
                        .overflow_hidden()
                        .flex_wrap()
                        .child(command),
                )
            })
            .into_any_element()
    }
}

impl Render for SelectionPopup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible_count = self.filtered.len();

        v_flex()
            .size_full()
            .bg(rgba(0x181818f5))
            .border_1()
            .border_color(rgba(0xffffff22))
            .rounded_lg()
            .text_color(cx.theme().foreground)
            .font_family(cx.theme().font_family.clone())
            .overflow_hidden()
            .child(
                h_flex()
                    .items_center()
                    .pb_2()
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        Input::new(&self.query)
                            .cleanable(true)
                            .border_0()
                            .w(px(200.0))
                            .tab_index(0)
                            .prefix(Icon::new(IconName::Search)),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("close-selection-popup")
                            .icon(IconName::Close)
                            .border_0()
                            .bg(rgba(0x00000000))
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                    ),
            )
            .child(h_flex().px_3().pb_2().text_sm().opacity(0.7).child(format!(
                "{visible_count} matches · ↑/↓ select · Enter open · Esc close"
            )))
            .child(
                v_flex()
                    .id("selection-popup-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .vertical_scrollbar(&self.scroll_handle)
                    .when(visible_count == 0, |list| {
                        list.child(
                            div()
                                .py_8()
                                .text_center()
                                .text_color(rgba(0xffffff88))
                                .child("No matching command stickers"),
                        )
                    })
                    .children((0..visible_count).map(|index| self.row(index, cx))),
            )
    }
}

fn clear_popup_handle() {
    if let Ok(mut popup) = SELECTION_POPUP.write() {
        *popup = None;
    }
}

fn close_popup(cx: &mut App) {
    let popup = SELECTION_POPUP
        .write()
        .ok()
        .and_then(|mut popup| popup.take());
    if let Some(popup) = popup {
        let _ = popup.update(cx, |_, window, _| window.remove_window());
    }
}

fn filtered_sticker_indices(stickers: &[StickerDetail], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    stickers
        .iter()
        .enumerate()
        .filter_map(|(index, sticker)| {
            if query.is_empty()
                || sticker.title.to_lowercase().contains(&query)
                || sticker_command(sticker).to_lowercase().contains(&query)
            {
                Some(index)
            } else {
                None
            }
        })
        .collect()
}

fn sticker_label(sticker: &StickerDetail) -> String {
    let title = sticker.title.trim();
    if !title.is_empty() {
        return title.to_string();
    }

    let command = sticker_command(sticker);
    if command.is_empty() {
        return "Untitled command".to_string();
    }

    let mut label: String = command.chars().take(60).collect();
    if command.chars().count() > 60 {
        label.push('…');
    }
    label
}

fn sticker_command(sticker: &StickerDetail) -> String {
    serde_json::from_str::<CommandContent>(&sticker.content)
        .map(|content| {
            content
                .command
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::sticker::{StickerColor, StickerState, StickerType};

    fn command_sticker(id: i64, title: &str, command: &str) -> StickerDetail {
        let mut content = CommandContent::default();
        content.command = command.to_string();
        content.accept_selection = true;

        StickerDetail {
            id,
            title: title.to_string(),
            state: StickerState::Close,
            left: 10,
            top: 20,
            width: 300,
            height: 400,
            top_most: false,
            color: StickerColor::Yellow,
            sticker_type: StickerType::Command,
            content: serde_json::to_string(&content).unwrap(),
            created_at: 0,
            updated_at: 0,
            display_id: None,
        }
    }

    #[test]
    fn empty_filter_preserves_lru_order() {
        let stickers = vec![
            command_sticker(3, "Third", "echo third"),
            command_sticker(1, "First", "echo first"),
        ];

        assert_eq!(filtered_sticker_indices(&stickers, ""), vec![0, 1]);
    }

    #[test]
    fn filter_matches_title_and_command_case_insensitively() {
        let stickers = vec![
            command_sticker(1, "Deploy Production", "cargo build"),
            command_sticker(2, "Build", "git STATUS --short"),
        ];

        assert_eq!(filtered_sticker_indices(&stickers, "production"), vec![0]);
        assert_eq!(filtered_sticker_indices(&stickers, "status"), vec![1]);
    }

    #[test]
    fn label_falls_back_to_compact_command() {
        let sticker = command_sticker(1, "  ", "git   status\n--short");

        assert_eq!(sticker_label(&sticker), "git status --short");
    }
}
