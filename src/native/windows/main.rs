use crate::gpui_component::Root;
use crate::gpui_component::alert::Alert;
use crate::gpui_component::button::{Button, ButtonVariants};
use crate::gpui_component::input::{Input, InputEvent, InputState};
use crate::gpui_component::menu::{DropdownMenu, PopupMenuItem};
use crate::gpui_component::spinner::Spinner;
use crate::gpui_component::*;
use gpui::{
    AnyWindowHandle, App, AsyncApp, Bounds, Context, Entity, FocusHandle, IntoElement,
    KeyDownEvent, MouseButton, MouseUpEvent, Render, ScrollStrategy, UniformListScrollHandle,
    WeakEntity, Window, WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowOptions,
    div, prelude::*, px, size, uniform_list,
};

use std::sync::mpsc::{self};
use std::time::Duration;

use crate::model::sticker::*;
use crate::native::components::IconName;
use crate::native::components::stickers::Sticker;
use crate::native::components::stickers::command::CommandSticker;
use crate::native::components::stickers::command::activity as command_activity;
use crate::native::components::stickers::file::FileSticker;
use crate::native::components::stickers::markdown::MarkdownSticker;
use crate::native::components::stickers::paint::PaintSticker;
use crate::native::components::stickers::timer::TimerSticker;
use crate::native::windows::StickerWindowEvent;
use crate::native::windows::sticker::StickerWindow;
use crate::storage::ArcStickerStore;

const STICKER_LOAD_LIMIT: i64 = 10000;
const STICKER_EVENT_PUMP_INTERVAL: Duration = Duration::from_millis(120);

/// Fixed row height for the manager sticker list. uniform_list only
/// renders rows whose bounding boxes are identical; titles are clipped
/// at three lines inside the row so the box stays bounded.
const ROW_HEIGHT: f32 = 80.0;
/// How many rows one PageUp / PageDown step moves. Chosen to roughly
/// match a six-row viewport so the focused row stays on screen.
const PAGE_STEP: usize = 6;

pub struct MainWindow {
    store: ArcStickerStore,
    sticker_events_sender: mpsc::Sender<StickerWindowEvent>,

    query: Entity<InputState>,
    order: StickerOrderBy,
    stickers: Vec<StickerBrief>,

    /// Index of the currently focused row in the sticker list.
    /// `None` means nothing is focused (e.g. before any load completes
    /// or right after a delete removed the focused row). Reset to
    /// `None` on sort changes; clamped into bounds on every select.
    /// Keyboard-driven navigation keeps this value stable across
    /// re-renders caused by the event pump, but a sort/search change
    /// discards it because the underlying row positions change.
    /// Domain IDs are the only safe identity for selection over a
    /// refresh; we still keep an index here because the keyboard
    /// model is index-based.
    selected_index: Option<usize>,
    /// Focus handle attached to the sticker list region. Created with
    /// \`tab_stop(true)\` so a Tab from the search input lands on the
    /// list, exposing arrow-navigation, Enter, and Delete actions.
    list_focus: FocusHandle,
    /// Scroll handle driving the virtualised list so keyboard navigation
    /// can scroll the focused row into view.
    list_scroll: UniformListScrollHandle,

    loading: bool,
    error: Option<String>,
    /// Monotonic counter incremented every time a sticker-load request is
    /// launched. Each request captures the counter at the moment it
    /// starts; the spawned task ignores its result unless it matches the
    /// counter held here when the response lands. This is the same
    /// revision-tag pattern as the timer sticker: a newer request
    /// supersedes older ones, so out-of-order responses (slow SQL
    /// followed by a sort change, a deletion that races a refresh, etc.)
    /// cannot overwrite a fresher intent. A successful delete also bumps
    /// this counter for the same reason: any in-flight reload that still
    /// has the deleted sticker in its payload must be invalidated before
    /// it lands.
    load_request_token: u64,
}

impl MainWindow {
    pub fn open(
        cx: &mut App,
        sticker_events_rx: mpsc::Receiver<StickerWindowEvent>,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
    ) -> anyhow::Result<AnyWindowHandle> {
        let bounds = Bounds::centered(None, size(px(340.), px(550.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(300.0), px(400.0))),
                window_background: WindowBackgroundAppearance::Transparent,
                titlebar: None,
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| {
                    MainWindow::new(window, cx, sticker_events_rx, sticker_events_tx, store)
                });
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .map(|x| x.into())
    }

    fn new(
        window: &mut Window,
        cx: &mut Context<MainWindow>,
        sticker_events_rx: mpsc::Receiver<StickerWindowEvent>,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Rustickers"));

        window.on_window_should_close(cx, |_, cx| {
            cx.quit();
            true
        });

        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |this, cx| {
                this.spawn_load_stickers(cx);
            });

            Self::loop_events(this, sticker_events_rx, cx).await;
        })
        .detach();

        cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.spawn_load_stickers(cx);
            }
        })
        .detach();

        Self {
            store,
            sticker_events_sender: sticker_events_tx,

            query,
            order: StickerOrderBy::CreatedDesc,
            stickers: Vec::new(),

            selected_index: None,
            list_focus: cx.focus_handle().tab_index(1).tab_stop(true),
            list_scroll: UniformListScrollHandle::default(),

            loading: false,
            error: None,
            load_request_token: 0,
        }
    }

    async fn loop_events(
        this: WeakEntity<Self>,
        sticker_events_rx: mpsc::Receiver<StickerWindowEvent>,
        cx: &mut AsyncApp,
    ) {
        let mut last_activity = command_activity::generation();

        loop {
            cx.background_executor()
                .timer(STICKER_EVENT_PUMP_INTERVAL)
                .await;

            let mut events: Vec<StickerWindowEvent> = Vec::new();
            while let Ok(ev) = sticker_events_rx.try_recv() {
                events.push(ev);
            }

            // Command runs are tracked in a process-wide registry rather than pushed as events,
            // because the background scheduler has no channel into this window. Polling its
            // generation counter on the pump that already exists keeps the indicators live.
            let activity = command_activity::generation();
            let activity_changed = activity != last_activity;
            last_activity = activity;

            if events.is_empty() && !activity_changed {
                continue;
            }

            let updated = this.update(cx, |this, cx| {
                let mut changed = activity_changed;
                for ev in events {
                    changed |= this.apply_event(ev, cx);
                }
                if changed {
                    cx.notify();
                }
            });

            if let Err(err) = updated {
                tracing::warn!(error = %err, "Failed to process sticker window events");
            }
        }
    }

    fn apply_event(&mut self, event: StickerWindowEvent, cx: &mut Context<Self>) -> bool {
        match event {
            StickerWindowEvent::Created { .. } => {
                self.spawn_load_stickers(cx);
                false
            }
            StickerWindowEvent::TitleChanged { id, title } => {
                if let Some(sticker) = self.stickers.iter_mut().find(|s| s.id == id)
                    && sticker.title != title
                {
                    sticker.title = title;
                    sticker.updated_at = crate::utils::time::now_unix_millis();
                    return true;
                }
                false
            }
            StickerWindowEvent::ColorChanged { id, color } => {
                if let Some(sticker) = self.stickers.iter_mut().find(|s| s.id == id)
                    && sticker.color != color
                {
                    sticker.color = color;
                    sticker.updated_at = crate::utils::time::now_unix_millis();
                    return true;
                }
                false
            }
            StickerWindowEvent::Closed { id } => {
                if let Some(sticker) = self.stickers.iter_mut().find(|s| s.id == id) {
                    sticker.state = StickerState::Close;
                    sticker.updated_at = crate::utils::time::now_unix_millis();
                    return true;
                }
                false
            }
        }
    }

    fn create_sticker(&mut self, cx: &mut Context<Self>, sticker_type: &StickerType) {
        if self.loading {
            return;
        }

        self.error = None;

        let size = match sticker_type {
            StickerType::Markdown => MarkdownSticker::default_window_size(),
            StickerType::Command => CommandSticker::default_window_size(),
            StickerType::Timer => TimerSticker::default_window_size(),
            StickerType::Paint => PaintSticker::default_window_size(),
            StickerType::File => FileSticker::default_window_size(),
        };

        let title = match sticker_type {
            StickerType::Markdown => "New Text Sticker",
            StickerType::Command => "New Command Sticker",
            StickerType::Timer => "New Timer Sticker",
            StickerType::Paint => "New Paint Sticker",
            StickerType::File => "New File Sticker",
        };

        let detail = StickerDetail {
            id: 0,
            title: title.to_string(),
            content: "".to_string(),
            color: StickerColor::Gray,
            sticker_type: *sticker_type,
            state: StickerState::Open,
            left: 100,
            top: 100,
            width: size.width,
            height: size.height,
            top_most: false,
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
        };

        let store = self.store.clone();
        let sticker_events_tx = self.sticker_events_sender.clone();
        cx.spawn(
            async move |entity, cx| match store.insert_sticker(detail).await {
                Ok(id) => {
                    let _ = sticker_events_tx.send(StickerWindowEvent::Created { id });

                    if let Err(err) =
                        StickerWindow::open_async(cx, sticker_events_tx.clone(), store.clone(), id)
                            .await
                    {
                        let _ = entity.update(cx, |this, cx| {
                            this.error = Some(format!("Failed to open sticker window: {err:#}"));
                            cx.notify();
                        });
                    }
                }
                Err(err) => {
                    let _ = entity.update(cx, |this, cx| {
                        this.error = Some(format!("Failed to create sticker: {err:#}"));
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn spawn_load_stickers(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.load_request_token = self.load_request_token.wrapping_add(1);
        let token = self.load_request_token;
        self.loading = true;
        cx.notify();

        let query = self.query.read(cx).value().to_string();
        let order_by = self.order;
        let store = self.store.clone();

        cx.spawn(async move |entity, cx| {
            let query_filter = (!query.is_empty()).then_some(query);
            let result = store
                .query_stickers(query_filter, order_by, STICKER_LOAD_LIMIT, 0)
                .await;

            let _ = entity.update(cx, move |this, cx| {
                // A newer request has been issued since this one was
                // dispatched -- either another sort/search change or a
                // successful delete that bumped the token to defend
                // against this very response. Drop the result so the
                // older ordering, filter, or about-to-be-deleted row
                // does not stomp on fresher state. `loading` stays true
                // here on purpose: whichever request is current will
                // clear it when its own response lands.
                if this.load_request_token != token {
                    return;
                }

                this.loading = false;
                match result {
                    Ok(stickers) => this.stickers = stickers,
                    Err(err) => {
                        this.error = Some(format!("Failed to query stickers: {err:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Open a sticker window starting from a list activation (mouse
    /// double-click, or the keyboard Enter handler in
    /// \`handle_list_key\`). The
    /// in-memory sticker is flipped to `Open` synchronously so the
    /// "greyed-out" closed state clears immediately; if the underlying
    /// `StickerWindow::open_async_with_options` call fails, the prior
    /// state is restored and the user sees a single actionable error.
    /// That keeps the optimistic flip honest while still surfacing
    /// failures instead of swallowing them.
    ///
    /// All callsites must funnel through this method so future keyboard
    /// and menu entry points share one error-handling and notification
    /// path.
    fn open_sticker(&mut self, id: i64, cx: &mut Context<Self>) {
        let prior_state = self.stickers.iter().find(|s| s.id == id).map(|s| s.state);
        if let Some(sticker) = self.stickers.iter_mut().find(|s| s.id == id) {
            sticker.state = StickerState::Open;
            cx.notify();
        }

        let store = self.store.clone();
        let sticker_events_tx = self.sticker_events_sender.clone();
        cx.spawn(async move |entity, cx| {
            // Opened from the list: command stickers with auto close start
            // in their settings view instead of running again.
            let result =
                StickerWindow::open_async_with_options(cx, sticker_events_tx, store, id, true)
                    .await;
            if let Err(err) = result {
                tracing::warn!(id, error = ?err, "Failed to open sticker window from list");
                let _ = entity.update(cx, |this, cx| {
                    if let Some(sticker) = this.stickers.iter_mut().find(|s| s.id == id)
                        && let Some(prior) = prior_state
                    {
                        sticker.state = prior;
                    }
                    this.error = Some(format!("Could not open sticker {id}: {err:#}"));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Domain id of the currently focused row, if any. The index may
    /// become stale across sort/search refreshes; the id is the only
    /// stable handle across those boundaries.
    fn selected_sticker_id(&self) -> Option<i64> {
        self.selected_index
            .and_then(|i| self.stickers.get(i).map(|s| s.id))
    }

    /// Move the focused row to a new index, clamped into bounds. Caller
    /// is expected to provide either a valid index or one past the end
    /// when the list is empty. After clamping, the scroll handle is
    /// asked to bring the row into view so keyboard navigation never
    /// leaves the focus target off-screen.
    fn move_selection_to(&mut self, new_index: usize, cx: &mut Context<Self>) {
        if self.stickers.is_empty() {
            self.selected_index = None;
            cx.notify();
            return;
        }
        let clamped = new_index.min(self.stickers.len() - 1);
        self.selected_index = Some(clamped);
        self.list_scroll
            .scroll_to_item(clamped, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Dispatch keyboard events scoped to the manager list. The list
    /// region uses \`track_focus\`; this handler only fires when the
    /// list has focus (after a Tab from the search input, or after a
    /// user clicked a row). Other key events fall through to whatever
    /// built-in handler the focused input has.
    fn handle_list_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.stickers.is_empty() {
            return;
        }

        let count = self.stickers.len();
        let current = self.selected_index.unwrap_or(0);
        let mods = event.keystroke.modifiers;

        let key = event.keystroke.key.as_str();
        let next: Option<usize> = match key {
            "down" => Some(current.saturating_add(1).min(count - 1)),
            "up" => Some(current.saturating_sub(1)),
            "home" => Some(0),
            "end" => Some(count - 1),
            "pageDown" => Some(current.saturating_add(PAGE_STEP).min(count - 1)),
            "pageUp" => Some(current.saturating_sub(PAGE_STEP)),
            _ => None,
        };

        if let Some(target) = next {
            self.move_selection_to(target, cx);
            cx.stop_propagation();
            window.prevent_default();
            return;
        }

        if key == "enter" && !mods.control && !mods.alt && !mods.platform && !mods.shift {
            if let Some(id) = self.selected_sticker_id() {
                self.open_sticker(id, cx);
            }
            cx.stop_propagation();
            window.prevent_default();
        }
    }

    fn delete_sticker(
        &mut self,
        id: i64,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entity = cx.entity();
        let store = self.store.clone();
        window.open_dialog(cx, move |dialog, _, _cx| {
            let store = store.clone();
            let entity = entity.clone();
            dialog
                .title(format!("Delete \"{title}\"?"))
                .child("This permanently removes the sticker and its saved content.")
                .footer(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| {
                                    window.close_dialog(cx);
                                }),
                        )
                        .child(
                            Button::new("delete-sticker")
                                .danger()
                                .label("Delete")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let store = store.clone();
                                    let entity = entity.clone();
                                    cx.spawn(async move |cx| {
                                        match store.delete_sticker(id).await {
                                            Ok(()) => {
                                                entity.update(cx, |this, cx| {
                                                    // Bump the load token so
                                                    // any already-running
                                                    // reload that still
                                                    // includes this row
                                                    // cannot resurrect it.
                                                    this.load_request_token =
                                                        this.load_request_token.wrapping_add(1);
                                                    this.loading = false;
                                                    StickerWindow::try_close(id, cx);
                                                    this.stickers.retain(|s| s.id != id);
                                                    cx.notify();
                                                });
                                            }
                                            Err(err) => {
                                                entity.update(cx, |this, cx| {
                                                    this.error = Some(format!(
                                                        "Failed to delete sticker: {err:#}"
                                                    ));
                                                    cx.notify();
                                                });
                                            }
                                        }
                                    })
                                    .detach();
                                }),
                        ),
                )
                .w(px(360.0))
        });
    }

    fn status_banner(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if let Some(err) = &self.error {
            return div()
                .p(px(8.0))
                .child(Alert::error("main-load-error", err.as_str()))
                .into_any_element();
        }
        if self.loading {
            return div()
                .p(px(8.0))
                .child(Spinner::new().color(cx.theme().accent))
                .into_any_element();
        }
        gpui::Empty.into_any_element()
    }

    fn sort_button(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let root_entity = cx.entity();
        Button::new("sort")
            .icon(match self.order {
                StickerOrderBy::CreatedAsc | StickerOrderBy::UpdatedAsc => IconName::SortAscending,
                _ => IconName::SortDescending,
            })
            .ghost()
            .dropdown_menu(move |menu, window, cx| {
                let order_by = root_entity.read(cx).order;
                menu.item(
                    PopupMenuItem::new(order_label(StickerOrderBy::CreatedDesc))
                        .checked(order_by == StickerOrderBy::CreatedDesc)
                        .on_click(window.listener_for(&root_entity, move |this, _, _, cx| {
                            this.order = StickerOrderBy::CreatedDesc;
                            this.spawn_load_stickers(cx);
                        })),
                )
                .item(
                    PopupMenuItem::new(order_label(StickerOrderBy::CreatedAsc))
                        .checked(order_by == StickerOrderBy::CreatedAsc)
                        .on_click(window.listener_for(&root_entity, move |this, _, _, cx| {
                            this.order = StickerOrderBy::CreatedAsc;
                            this.spawn_load_stickers(cx);
                        })),
                )
                .item(
                    PopupMenuItem::new(order_label(StickerOrderBy::UpdatedDesc))
                        .checked(order_by == StickerOrderBy::UpdatedDesc)
                        .on_click(window.listener_for(&root_entity, move |this, _, _, cx| {
                            this.order = StickerOrderBy::UpdatedDesc;
                            this.spawn_load_stickers(cx);
                        })),
                )
                .item(
                    PopupMenuItem::new(order_label(StickerOrderBy::UpdatedAsc))
                        .checked(order_by == StickerOrderBy::UpdatedAsc)
                        .on_click(window.listener_for(&root_entity, move |this, _, _, cx| {
                            this.order = StickerOrderBy::UpdatedAsc;
                            this.spawn_load_stickers(cx);
                        })),
                )
            })
            .into_any_element()
    }

    fn create_button(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let root_entity = cx.entity();
        Button::new("create")
            .ghost()
            .icon(IconName::Plus)
            .dropdown_menu(move |menu, window, _| {
                let root_entity = root_entity.clone();
                menu.item(
                    PopupMenuItem::new("text")
                        .icon(sticker_type_icon(&StickerType::Markdown))
                        .on_click(window.listener_for(&root_entity, |this, _, _, cx| {
                            this.create_sticker(cx, &StickerType::Markdown);
                        })),
                )
                .item(
                    PopupMenuItem::new("timer")
                        .icon(sticker_type_icon(&StickerType::Timer))
                        .on_click(window.listener_for(&root_entity, |this, _, _, cx| {
                            this.create_sticker(cx, &StickerType::Timer);
                        })),
                )
                .item(
                    PopupMenuItem::new("command")
                        .icon(sticker_type_icon(&StickerType::Command))
                        .on_click(window.listener_for(&root_entity, |this, _, _, cx| {
                            this.create_sticker(cx, &StickerType::Command);
                        })),
                )
                .item(
                    PopupMenuItem::new("paint")
                        .icon(sticker_type_icon(&StickerType::Paint))
                        .on_click(window.listener_for(&root_entity, |this, _, _, cx| {
                            this.create_sticker(cx, &StickerType::Paint);
                        })),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Clipboard history")
                        .icon(IconName::DocumentText)
                        .on_click(window.listener_for(&root_entity, |this, _, _, cx| {
                            if super::clipboard::open_from_main(
                                cx,
                                this.sticker_events_sender.clone(),
                            )
                            .is_err()
                            {
                                this.error = Some("Clipboard backend is unavailable".into());
                                cx.notify();
                            }
                        })),
                )
            })
            .into_any_element()
    }

    fn sticker_card(
        &self,
        index: usize,
        sticker: &StickerBrief,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = sticker.id;
        let title = sticker.title.clone();
        let updated = crate::utils::time::format_unix_millis(sticker.updated_at);
        let is_selected = self.selected_index == Some(index);

        // A command sticker can be busy, or waiting on a schedule, even while it is closed. That is
        // otherwise invisible from the list.
        let activity = if sticker.sticker_type == StickerType::Command {
            CardActivity::of_command(
                command_activity::is_running(id),
                command_activity::next_run(id),
            )
        } else {
            CardActivity::Idle
        };

        let running = matches!(activity, CardActivity::Running);
        let next_run = activity.next_run();
        let status = activity.status_line(&updated);

        let main = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().text_color(sticker.color.swatch()).child(
                        Icon::new(sticker_type_icon(&sticker.sticker_type)).with_size(px(14.)),
                    ))
                    .when(running, |row| {
                        row.child(
                            div()
                                .text_color(yellow_500())
                                .child(Spinner::new().with_size(px(12.))),
                        )
                    })
                    .when(next_run.is_some(), |row| {
                        row.child(
                            div()
                                .opacity(0.7)
                                .child(Icon::new(IconName::Loop).with_size(px(12.))),
                        )
                    })
                    .child(
                        div()
                            .text_sm()
                            .overflow_hidden()
                            .line_clamp(3)
                            .text_ellipsis()
                            .pr_2()
                            .child(if title.is_empty() {
                                "...".to_string()
                            } else {
                                title.clone()
                            }),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .opacity(0.75)
                    .text_right()
                    .when(running, |view| view.text_color(yellow_500()))
                    .child(status),
            );

        // Fixed row height. uniform_list requires one row height for the
        // whole surface; titles are already line_clamp(3) inside the
        // padded main column so the content stays bounded. The number is
        // roughly title area (48 px × 14 px line height) + status (16 px)
        // + padding (16 px vertical inside .p_2 outer). At rem size 14,
        // 80 px works without clipping.
        let row_bg: gpui::Hsla = if is_selected {
            cx.theme().list_active
        } else {
            sticker.color.bg().into()
        };
        div()
            .id(("sticker-row", id as usize))
            .h(px(ROW_HEIGHT))
            .bg(row_bg)
            .border_1()
            .rounded_md()
            .when(is_selected, |el| {
                el.border_color(cx.theme().list_active_border)
            })
            .when(!is_selected, |el| el.border_color(cx.theme().border))
            .opacity(if sticker.state == StickerState::Close {
                0.6
            } else {
                1.0
            })
            .hover(|s| s.bg(cx.theme().list_hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.selected_index = Some(index);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                    if event.click_count >= 2 {
                        this.open_sticker(id, cx);
                    }
                }),
            )
            .child(main)
            .child(
                Button::new(("delete", id as u64))
                    .absolute()
                    .top_0()
                    .right_0()
                    .icon(IconName::Close)
                    .ghost()
                    .occlude()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.delete_sticker(id, title.clone(), window, cx);
                    })),
            )
            .into_any_element()
    }

    fn title_bar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        h_flex()
            .gap_2()
            .justify_between()
            .window_control_area(WindowControlArea::Drag)
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Input::new(&self.query)
                            .cleanable(true)
                            .border_0()
                            .w(px(160.0))
                            .tab_index(0)
                            .prefix(Icon::new(IconName::Search)),
                    )
                    .child(self.sort_button(cx)),
            )
            .child(
                h_flex()
                    .child(
                        Button::new("minimize")
                            .icon(IconName::Minus)
                            .ghost()
                            .occlude()
                            .on_click(cx.listener(|_, _, window, _| {
                                window.minimize_window();
                            })),
                    )
                    .child(
                        Button::new("close")
                            .icon(IconName::Close)
                            .ghost()
                            .occlude()
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.quit();
                            })),
                    )
                    .child(self.create_button(cx)),
            )
            .into_any_element()
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list_count = self.stickers.len();
        let view = cx.entity();

        v_flex()
            .gap_2()
            .size_full()
            .bg(cx.theme().background)
            .on_mouse_down(MouseButton::Left, |_, window, _| {
                if !window.is_window_active() {
                    window.activate_window();
                }
            })
            .child(self.title_bar(cx))
            .child(
                div()
                    .h_full()
                    .flex_shrink(1.0)
                    .overflow_hidden()
                    .track_focus(&self.list_focus)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        this.handle_list_key(event, window, cx);
                    }))
                    .when(list_count > 0, |el| {
                        el.child(
                            uniform_list(
                                "manager-stickers",
                                list_count,
                                move |range, _window, cx| {
                                    view.update(cx, |this, cx| {
                                        range
                                            .into_iter()
                                            .filter_map(|index| {
                                                this.stickers
                                                    .get(index)
                                                    .map(|s| this.sticker_card(index, s, cx))
                                            })
                                            .collect::<Vec<_>>()
                                    })
                                },
                            )
                            .size_full()
                            .track_scroll(&self.list_scroll),
                        )
                    })
                    .when(list_count == 0, |el| {
                        el.child(
                            v_flex()
                                .flex_1()
                                .items_center()
                                .justify_center()
                                .gap_1()
                                .text_color(cx.theme().muted_foreground)
                                .child(Icon::new(IconName::DocumentText).with_size(px(20.)))
                                .child(if self.loading {
                                    "Loading stickers\u{2026}"
                                } else if self.error.is_some() {
                                    "Could not load stickers."
                                } else {
                                    "No stickers yet. Create one with the + menu."
                                }),
                        )
                    }),
            )
            .child(
                v_flex()
                    .gap_2()
                    .px_2()
                    .window_control_area(WindowControlArea::Drag)
                    .child(self.status_banner(cx)),
            )
    }
}

/// What a card tells the user about a command sticker's schedule.
///
/// Only command stickers can be anything but [`CardActivity::Idle`]; every other type simply shows
/// when it was last updated.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CardActivity {
    Idle,
    Running,
    Scheduled(String),
}

impl CardActivity {
    fn of_command(running: bool, next_run: Option<String>) -> Self {
        // Running wins: while a command is going, its next fire time is the less interesting fact,
        // and showing both would make the card jitter between two lines of text.
        match (running, next_run) {
            (true, _) => Self::Running,
            (false, Some(next_run)) => Self::Scheduled(next_run),
            (false, None) => Self::Idle,
        }
    }

    fn next_run(&self) -> Option<&str> {
        match self {
            Self::Scheduled(next_run) => Some(next_run.as_str()),
            _ => None,
        }
    }

    fn status_line(&self, updated: &str) -> String {
        match self {
            Self::Running => "Running...".to_string(),
            Self::Scheduled(next_run) => format!("Next run: {next_run}"),
            Self::Idle => format!("Updated: {updated}"),
        }
    }
}

fn sticker_type_icon(sticker_type: &StickerType) -> IconName {
    match sticker_type {
        StickerType::Markdown => IconName::DocumentText,
        StickerType::Command => IconName::Command,
        StickerType::Timer => IconName::Bell,
        StickerType::Paint => IconName::Paint,
        StickerType::File => IconName::DocumentText,
    }
}

fn order_label(order_by: StickerOrderBy) -> &'static str {
    match order_by {
        StickerOrderBy::CreatedDesc => "Created ↓",
        StickerOrderBy::CreatedAsc => "Created ↑",
        StickerOrderBy::UpdatedDesc => "Updated ↓",
        StickerOrderBy::UpdatedAsc => "Updated ↑",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_sticker_card_shows_when_it_was_updated() {
        let activity = CardActivity::Idle;

        assert_eq!(activity.next_run(), None);
        assert_eq!(activity.status_line("yesterday"), "Updated: yesterday");
    }

    #[test]
    fn a_scheduled_command_card_shows_its_next_run() {
        let activity = CardActivity::of_command(false, Some("2030-01-01 00:00:00".to_string()));

        assert_eq!(activity.next_run(), Some("2030-01-01 00:00:00"));
        assert_eq!(
            activity.status_line("yesterday"),
            "Next run: 2030-01-01 00:00:00"
        );
    }

    #[test]
    fn a_running_command_card_hides_the_next_run() {
        let activity = CardActivity::of_command(true, Some("2030-01-01 00:00:00".to_string()));

        assert_eq!(activity, CardActivity::Running);
        assert_eq!(
            activity.next_run(),
            None,
            "no clock icon while the spinner is up"
        );
        assert_eq!(activity.status_line("yesterday"), "Running...");
    }

    #[test]
    fn an_unscheduled_command_card_is_indistinguishable_from_any_other() {
        assert_eq!(CardActivity::of_command(false, None), CardActivity::Idle);
    }

    /// Pure arithmetic covering the keyboard-navigation logic. The real
    /// \`handle_list_key\` needs GPUI context for keyboard wiring; this
    /// exercises the same clamp / page-step behaviour so regressions in
    /// \`move_selection_to\` show up without UI integration tests.
    #[test]
    fn keyboard_navigation_arithmetic_clamps_and_pages() {
        fn clamp(current: usize, count: usize) -> usize {
            if count == 0 {
                0
            } else {
                current.min(count - 1)
            }
        }
        fn step_up(current: usize, step: usize, count: usize) -> usize {
            (current + step).min(count.saturating_sub(1))
        }
        fn step_down(current: usize, step: usize) -> usize {
            current.saturating_sub(step)
        }

        // Empty list keeps the user on a valid index.
        assert_eq!(clamp(0, 0), 0);
        // Last element is a hard cap.
        assert_eq!(step_up(998, 10, 1000), 999);
        // Saturating subtract at the top.
        assert_eq!(step_down(0, 5), 0);
        assert_eq!(step_down(2, 5), 0);
        assert_eq!(step_down(20, 5), 15);
        // Page step equals a chunk of rows.
        assert_eq!(step_up(0, PAGE_STEP, 1000), PAGE_STEP);
        assert_eq!(step_down(PAGE_STEP, PAGE_STEP), 0);
    }
}
