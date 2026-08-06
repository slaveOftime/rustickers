use std::sync::{Arc, RwLock, mpsc};

use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, Bounds, Context, Entity, IntoElement, Render,
    ScrollHandle, Subscription, Window, WindowBackgroundAppearance, WindowBounds,
    WindowControlArea, WindowKind, WindowOptions, black, div, prelude::*, px, rgba, size,
    transparent_black,
};
use gpui_component::{
    Icon, Root,
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
    native::{SelectionCommandTarget, components::IconName, windows::StickerWindowEvent},
    storage::ArcStickerStore,
};

use super::{EscapeDismissTarget, set_escape_dismiss_target_active, sticker::StickerWindow};

const POPUP_WIDTH: f32 = 300.0;
const POPUP_HEIGHT: f32 = 400.0;
#[cfg(target_os = "macos")]
const INPUT_VIEW_HINT: &str = "Cmd+Enter newline · Enter confirm · Esc close";
#[cfg(not(target_os = "macos"))]
const INPUT_VIEW_HINT: &str = "Ctrl+Enter newline · Enter confirm · Esc close";

static SELECTION_POPUP: RwLock<Option<AnyWindowHandle>> = RwLock::new(None);

/// The view the popup currently shows. Both views live in the same window so
/// switching between them does not pay the cost of opening another window.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SelectionView {
    /// Manual text input, used when no direct selection was captured.
    Input,
    /// Command sticker chooser for the current text.
    Choose,
}

/// The view the popup starts in.
enum SelectionPopupInit {
    Choose {
        stickers: Vec<StickerDetail>,
        selection: String,
    },
    Input,
}

pub struct SelectionPopup {
    store: ArcStickerStore,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    stickers: Arc<Vec<StickerDetail>>,
    filtered: Vec<usize>,
    selected: usize,
    selection: Arc<str>,
    query: Entity<InputState>,
    input: Entity<InputState>,
    view: SelectionView,
    input_error: Option<String>,
    resolving: bool,
    scroll_handle: ScrollHandle,
    closing: bool,
    _keystroke_subscription: Subscription,
    _window_activation_subscription: Subscription,
}

impl SelectionPopup {
    /// Open the chooser for an already captured selection.
    pub fn open(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        stickers: Vec<StickerDetail>,
        selection: String,
    ) -> anyhow::Result<()> {
        Self::open_with(
            cx,
            sticker_events_tx,
            store,
            SelectionPopupInit::Choose {
                stickers,
                selection,
            },
        )
    }

    /// Open the manual text input, used when nothing is selected.
    pub fn open_for_input(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
    ) -> anyhow::Result<()> {
        Self::open_with(cx, sticker_events_tx, store, SelectionPopupInit::Input)
    }

    fn open_with(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        init: SelectionPopupInit,
    ) -> anyhow::Result<()> {
        let existing = SELECTION_POPUP
            .write()
            .ok()
            .and_then(|mut popup| popup.take());
        if let Some(existing) = existing {
            set_escape_dismiss_target_active(EscapeDismissTarget::Selection, false);
            let _ = existing.update(cx, |_, window, _| window.remove_window());
            cx.defer(move |cx| {
                if let Err(err) = Self::open_with(cx, sticker_events_tx, store, init) {
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
                let entity = cx.new(|cx| Self::new(window, cx, sticker_events_tx, store, init));
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
        init: SelectionPopupInit,
    ) -> Self {
        let (view, stickers, selection) = match init {
            SelectionPopupInit::Choose {
                stickers,
                selection,
            } => (SelectionView::Choose, stickers, selection),
            SelectionPopupInit::Input => (SelectionView::Input, Vec::new(), String::new()),
        };

        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Filter stickers ..."));
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .submit_on_enter(true)
                .placeholder("Type the text to send to a command sticker ...")
        });
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

        cx.subscribe_in(
            &input,
            window,
            |this, input, event, window, cx| match event {
                InputEvent::Change => {
                    if this.input_error.take().is_some() {
                        cx.notify();
                    }
                }
                // `Enter` confirms, `Ctrl/Cmd + Enter` inserts a newline.
                // `Shift + Enter` already inserted its own newline.
                InputEvent::PressEnter { secondary, shift } => {
                    if *secondary {
                        input.update(cx, |input, cx| input.insert("\n", window, cx));
                    } else if !shift {
                        this.confirm_input(window, cx);
                    }
                }
                _ => {}
            },
        )
        .detach();

        let own_window_id = window.window_handle().window_id();
        let entity = cx.weak_entity();
        let keystroke_subscription = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle().window_id() != own_window_id || !window.is_window_active() {
                return;
            }
            let Some(entity) = entity.upgrade() else {
                return;
            };
            let handled = entity.update(cx, |this, cx| match event.keystroke.key.as_str() {
                "up" if this.view == SelectionView::Choose => {
                    this.move_selection(-1, cx);
                    true
                }
                "down" if this.view == SelectionView::Choose => {
                    this.move_selection(1, cx);
                    true
                }
                "escape" => {
                    this.dismiss(cx);
                    true
                }
                _ => false,
            });
            if handled {
                cx.stop_propagation();
                window.prevent_default();
            }
        });
        let window_activation_subscription =
            cx.observe_window_activation(window, |_, window, _| {
                set_escape_dismiss_target_active(
                    EscapeDismissTarget::Selection,
                    window.is_window_active(),
                );
            });

        window.on_window_should_close(cx, |_, _| {
            set_escape_dismiss_target_active(EscapeDismissTarget::Selection, false);
            clear_popup_handle();
            true
        });

        match view {
            SelectionView::Choose => query.update(cx, |query, cx| query.focus(window, cx)),
            SelectionView::Input => input.update(cx, |input, cx| input.focus(window, cx)),
        }

        Self {
            store,
            sticker_events_tx,
            stickers: Arc::new(stickers),
            filtered,
            selected: 0,
            selection: Arc::from(selection),
            query,
            input,
            view,
            input_error: None,
            resolving: false,
            scroll_handle: ScrollHandle::new(),
            closing: false,
            _keystroke_subscription: keystroke_subscription,
            _window_activation_subscription: window_activation_subscription,
        }
    }

    /// Confirm the manually typed text and switch this same window over to the
    /// sticker chooser instead of opening another window.
    fn confirm_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing || self.resolving {
            return;
        }

        let text = self.input.read(cx).value().to_string();

        self.resolving = true;
        self.input_error = None;
        cx.notify();

        tracing::info!(
            text_len = text.len(),
            "Manual selection input confirmed, resolving command stickers"
        );

        let store = self.store.clone();
        let entity = cx.weak_entity();
        window
            .spawn(cx, async move |cx| {
                let target = crate::native::resolve_selection_command(&store).await;
                let _ = entity.update_in(cx, |this, window, cx| {
                    this.resolving = false;
                    match target {
                        Ok(SelectionCommandTarget::Single(sticker)) => {
                            this.show_stickers(text, vec![sticker], window, cx);
                            this.open_selected(cx);
                        }
                        Ok(SelectionCommandTarget::Choose(stickers)) => {
                            this.show_stickers(text, stickers, window, cx);
                        }
                        Err(err) => {
                            tracing::warn!(error = ?err, "Failed to resolve command stickers for input");
                            this.input_error = Some(format!("{err:#}"));
                            cx.notify();
                        }
                    }
                });
            })
            .detach();
    }

    /// Switch the window from the input view to the chooser view.
    fn show_stickers(
        &mut self,
        selection: String,
        stickers: Vec<StickerDetail>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selection = Arc::from(selection);
        self.filtered = (0..stickers.len()).collect();
        self.stickers = Arc::new(stickers);
        self.selected = 0;
        self.view = SelectionView::Choose;
        self.scroll_handle.scroll_to_item(0);
        self.query.update(cx, |query, cx| query.focus(window, cx));
        cx.notify();
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
            .when(selected, |row| row.bg(rgba(0xe5c23696)))
            .hover(|row| row.bg(rgba(0xe5c23664)))
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

    fn choose_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let visible_count = self.filtered.len();

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .items_center()
                    .pb_2()
                    .window_control_area(WindowControlArea::Drag)
                    // The header is a window drag area, which swallows clicks
                    // unless the field occludes it.
                    .child(
                        div().occlude().child(
                            Input::new(&self.query)
                                .cleanable(true)
                                .border_0()
                                .w(px(200.0))
                                .tab_index(0)
                                .prefix(Icon::new(IconName::Search)),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("close-selection-popup")
                            .icon(IconName::Close)
                            .border_0()
                            .bg(rgba(0x00000000))
                            .cursor_pointer()
                            // The header is a window drag area, which swallows
                            // clicks unless the button occludes it.
                            .occlude()
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
            .into_any_element()
    }

    fn input_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let error = self.input_error.clone();

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .items_center()
                    .window_control_area(WindowControlArea::Drag)
                    .child(div().text_sm().px_2().child("Input and invoke ..."))
                    .child(div().flex_1())
                    .child(
                        Button::new("close-selection-popup")
                            .icon(IconName::Close)
                            .border_0()
                            .bg(rgba(0x00000000))
                            .cursor_pointer()
                            // The header is a window drag area, which swallows
                            // clicks unless the button occludes it.
                            .occlude()
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                    ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    Input::new(&self.input)
                        .size_full()
                        .p_2()
                        .bordered(false)
                        .bg(rgba(0x00000000)),
                ),
            )
            .child(
                h_flex()
                    .p_2()
                    .text_sm()
                    .opacity(0.6)
                    .when(error.is_some(), |row| row.text_color(rgba(0xf87171ff)))
                    .child(match &error {
                        Some(error) => format!("{error} · {INPUT_VIEW_HINT}"),
                        None if self.resolving => "Looking for command stickers ...".to_string(),
                        None => INPUT_VIEW_HINT.to_string(),
                    }),
            )
            .into_any_element()
    }
}

impl Render for SelectionPopup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(black().opacity(0.85))
            .overflow_hidden()
            .child(match self.view {
                SelectionView::Input => self.input_view(cx),
                SelectionView::Choose => self.choose_view(cx),
            })
    }
}

fn clear_popup_handle() {
    set_escape_dismiss_target_active(EscapeDismissTarget::Selection, false);
    if let Ok(mut popup) = SELECTION_POPUP.write() {
        *popup = None;
    }
}

pub fn close_popup(cx: &mut App) {
    set_escape_dismiss_target_active(EscapeDismissTarget::Selection, false);
    let popup = SELECTION_POPUP
        .write()
        .ok()
        .and_then(|mut popup| popup.take());
    if let Some(popup) = popup {
        let _ = popup.update(cx, |_, window, _| window.remove_window());
    }
}

pub fn dispatch_escape(cx: &mut App) -> bool {
    let escape = gpui::Keystroke::parse("escape").expect("escape is a valid GPUI keystroke");
    SELECTION_POPUP
        .read()
        .ok()
        .and_then(|popup| {
            popup.as_ref().and_then(|handle| {
                handle
                    .update(cx, |_, window, cx| window.dispatch_keystroke(escape, cx))
                    .ok()
            })
        })
        .unwrap_or(false)
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
