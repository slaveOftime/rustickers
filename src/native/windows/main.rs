use gpui::{
    AnyWindowHandle, App, AsyncApp, Bounds, Context, Entity, IntoElement, MouseButton,
    MouseUpEvent, Render, WeakEntity, Window, WindowBackgroundAppearance, WindowBounds,
    WindowControlArea, WindowOptions, div, prelude::*, px, rgb, rgba, size, transparent_black,
};
use gpui_component::Root;
use gpui_component::alert::Alert;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
use gpui_component::scroll::ScrollableElement;
use gpui_component::spinner::Spinner;
use gpui_component::*;

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

pub struct MainWindow {
    store: ArcStickerStore,
    sticker_events_sender: mpsc::Sender<StickerWindowEvent>,

    query: Entity<InputState>,
    order: StickerOrderBy,
    stickers: Vec<StickerBrief>,

    loading: bool,
    error: Option<String>,
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
                cx.new(|cx| Root::new(view, window, cx).bg(transparent_black().alpha(0.0)))
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

            loading: false,
            error: None,
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
        if self.loading {
            return;
        }

        self.loading = true;
        self.error = None;
        cx.notify();

        let query = self.query.read(cx).value().to_string();
        let order_by = self.order;
        let store = self.store.clone();

        cx.spawn(async move |entity, cx| {
            let query = (!query.is_empty()).then_some(query);
            let Ok(stickers) = store
                .query_stickers(query, order_by, STICKER_LOAD_LIMIT, 0)
                .await
            else {
                let _ = entity.update(cx, move |this, cx| {
                    this.error = Some("Failed to query stickers".to_string());
                    this.loading = false;
                    cx.notify();
                });
                return;
            };

            let _ = entity.update(cx, move |this, cx| {
                this.stickers = stickers;
                this.loading = false;
                cx.notify();
            });
        })
        .detach();
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
        window.open_dialog(cx, move |dialog, _, cx| {
            let store = store.clone();
            let entity = entity.clone();
            dialog
                .title(div().text_color(cx.theme().warning).child("Warning"))
                .child(format!("Are you confirm to delete: \"{title}\"?"))
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
                        .child(Button::new("Delete").warning().label("Delete").on_click(
                            move |_, window, cx| {
                                window.close_dialog(cx);
                                let store = store.clone();
                                let entity = entity.clone();
                                cx.spawn(async move |cx| match store.delete_sticker(id).await {
                                    Ok(()) => {
                                        entity.update(cx, |this, cx| {
                                            StickerWindow::try_close(id, cx);
                                            this.stickers.retain(|s| s.id != id);
                                        });
                                    }
                                    Err(err) => {
                                        entity.update(cx, |this, cx| {
                                            this.error =
                                                Some(format!("Failed to delete sticker: {err:#}"));
                                            cx.notify();
                                        });
                                    }
                                })
                                .detach();
                            },
                        )),
                )
                .w(px(300.0))
                .bg(black().opacity(0.9))
                .text_sm()
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
            .bg(rgba(0x00000000))
            .border_0()
            .opacity(0.8)
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
            .border_0()
            .bg(rgba(0x00000000))
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
            })
            .into_any_element()
    }

    fn sticker_card(sticker: &StickerBrief, cx: &mut Context<Self>) -> gpui::AnyElement {
        let id = sticker.id;
        let title = sticker.title.clone();
        let updated = crate::utils::time::format_unix_millis(sticker.updated_at);

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

        div()
            .bg(sticker.color.bg())
            .opacity(if sticker.state == StickerState::Close {
                0.6
            } else {
                1.0
            })
            .hover(|s| s.bg(rgb(0x333333)).cursor_pointer())
            .border_1()
            .border_color(rgb(0x3a3a3a))
            .rounded_md()
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                    if event.click_count >= 2 {
                        if let Some(sticker) = this.stickers.iter_mut().find(|s| s.id == id) {
                            sticker.state = StickerState::Open;
                        }
                        let store = this.store.clone();
                        let sticker_events_tx = this.sticker_events_sender.clone();
                        cx.spawn(async move |_, cx| {
                            // Opened from the list: command stickers with auto close start in
                            // their settings view instead of running again.
                            let _ = StickerWindow::open_async_with_options(
                                cx,
                                sticker_events_tx,
                                store,
                                id,
                                true,
                            )
                            .await;
                        })
                        .detach();
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
                    .border_0()
                    .bg(rgba(0x00000000))
                    .opacity(0.8)
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
                            .focus_ring(false)
                            .bg(rgba(0x00000000))
                            .prefix(Icon::new(IconName::Search)),
                    )
                    .child(self.sort_button(cx)),
            )
            .child(
                h_flex()
                    .child(
                        Button::new("minimize")
                            .icon(IconName::Minus)
                            .border_0()
                            .bg(rgba(0x00000000))
                            .opacity(0.8)
                            .occlude()
                            .on_click(cx.listener(|_, _, window, _| {
                                window.minimize_window();
                            })),
                    )
                    .child(
                        Button::new("close")
                            .icon(IconName::Close)
                            .border_0()
                            .bg(rgba(0x00000000))
                            .opacity(0.8)
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .size_full()
            .bg(black().opacity(0.85))
            .on_mouse_down(MouseButton::Left, |_, window, _| {
                if !window.is_window_active() {
                    window.activate_window();
                }
            })
            .child(self.title_bar(cx))
            .child(
                div().h_full().flex_shrink(1.0).overflow_hidden().child(
                    v_flex().overflow_y_scrollbar().children(
                        self.stickers
                            .iter()
                            .map(|s| div().pl_2().pr_2().pb_2().child(Self::sticker_card(s, cx))),
                    ),
                ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .px_2()
                    .window_control_area(WindowControlArea::Drag)
                    .child(self.status_banner(cx)),
            )
            .children(Root::render_dialog_layer(window, cx))
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
}
