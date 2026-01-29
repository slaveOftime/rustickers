use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Context, IntoElement,
    MouseButton, Render, SharedString, TitlebarOptions, Window, WindowBackgroundAppearance,
    WindowBounds, WindowControlArea, WindowOptions, div, prelude::*, px, rgba, size,
};
use gpui_component::{
    ActiveTheme, Root, TitleBar,
    alert::Alert,
    button::Button,
    h_flex,
    input::{InputEvent, InputState},
    v_flex,
};
use std::{
    sync::{RwLock, mpsc},
    time::{Duration, Instant},
};

use crate::{
    components::{
        IconName,
        stickers::{command::CommandSticker, markdown::MarkdownSticker, timer::TimerSticker, *},
    },
    model::sticker::{StickerColor, StickerDetail, StickerState, StickerType},
    storage::ArcStickerStore,
    windows::StickerWindowEvent,
};

const BOUNDS_SAVE_DEBOUNCE: Duration = Duration::from_millis(200);

static OPEN_STICKERS: RwLock<Vec<(i64, AnyWindowHandle)>> = RwLock::new(Vec::new());

pub struct StickerWindow {
    store: ArcStickerStore,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    detail: StickerDetail,

    view: Box<dyn StickerView>,
    error: Option<String>,

    last_bounds: Option<(i32, i32, i32, i32)>,
    last_bounds_change_at: Option<Instant>,
}

impl StickerWindow {
    pub async fn open_async(
        cx: &mut AsyncApp,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        id: i64,
    ) -> anyhow::Result<()> {
        if let Ok(open_stickers) = OPEN_STICKERS.read() {
            if let Some((_, handle)) = open_stickers.iter().find(|(open_id, _)| *open_id == id) {
                let _ = cx.update(|cx| {
                    handle.update(cx, |_, window, _| {
                        window.activate_window();
                    })
                })?;
                return Ok(());
            }
        }

        let detail = match store.get_sticker(id).await {
            Ok(detail) => detail,
            Err(err) => {
                return Err(anyhow::anyhow!("Failed to open sticker: {err:#}"));
            }
        };

        if detail.state != StickerState::Open
            && let Err(err) = store.update_sticker_state(id, StickerState::Open).await
        {
            return Err(anyhow::anyhow!(
                "Failed to update sticker state to open: {err:#}"
            ));
        }

        cx.update(|cx| Self::open_with_detail(cx, sticker_events_tx, store, detail))?
    }

    pub fn try_close(id: i64, cx: &mut App) -> bool {
        if let Ok(mut open_stickers) = OPEN_STICKERS.write() {
            if let Some(pos) = open_stickers.iter().position(|(open_id, _)| *open_id == id) {
                let (_, handle) = open_stickers.remove(pos);
                return handle
                    .update(cx, |_, window, _| {
                        window.remove_window();
                        true
                    })
                    .unwrap_or(false);
            }
        }
        false
    }

    fn open_with_detail(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        detail: StickerDetail,
    ) -> anyhow::Result<()> {
        let id = detail.id;
        if let Ok(open_stickers) = OPEN_STICKERS.read() {
            if let Some((_, handle)) = open_stickers.iter().find(|(open_id, _)| *open_id == id) {
                handle.update(cx, |_, window, _| {
                    window.activate_window();
                })?;
                return Ok(());
            }
        }

        let min_size = match detail.sticker_type {
            StickerType::Timer => TimerSticker::min_window_size(),
            StickerType::Markdown => MarkdownSticker::min_window_size(),
            StickerType::Command => CommandSticker::min_window_size(),
        };

        let current_size = if detail.width > 0 && detail.height > 0 {
            size(detail.width, detail.height)
        } else {
            match detail.sticker_type {
                StickerType::Timer => TimerSticker::default_window_size(),
                StickerType::Markdown => MarkdownSticker::default_window_size(),
                StickerType::Command => CommandSticker::default_window_size(),
            }
        };

        let bounds = Bounds::from_corner_and_size(
            gpui::Corner::TopLeft,
            gpui::point(px(detail.left as f32), px(detail.top as f32)),
            current_size.map(|x| px(x as f32)),
        );

        let handle = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(min_size.map(|x| px(x as f32))),
                window_background: WindowBackgroundAppearance::Transparent,
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::new(detail.title.clone())),
                    ..TitleBar::title_bar_options()
                }),
                ..Default::default()
            },
            |window, cx| {
                let view =
                    cx.new(|cx| StickerWindow::new(detail, store, sticker_events_tx, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )?;

        if let Ok(mut open_stickers) = OPEN_STICKERS.write() {
            open_stickers.push((id, handle.into()));
        }

        Ok(())
    }

    fn new(
        detail: StickerDetail,
        store: ArcStickerStore,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        window: &mut Window,
        cx: &mut Context<StickerWindow>,
    ) -> Self {
        let title_val = detail.title.clone();
        let title = cx.new(|cx| InputState::new(window, cx).default_value(title_val));

        let mut view =
            Self::create_sticker_view(&detail, &store, window, cx, sticker_events_tx.clone());

        view.set_color(cx, detail.color);

        cx.subscribe_in(&title, window, |this, input_state, event, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let id = this.detail.id;
                let text = input_state.read(cx).value().to_string();
                let store = this.store.clone();
                let events = this.sticker_events_tx.clone();
                cx.spawn(async move |entity, cx| {
                    if let Err(err) = store.update_sticker_title(id, text.clone()).await {
                        let _ = entity.update(cx, |this, cx| {
                            this.set_error(format!("Failed to save title: {err}"), cx);
                        });
                    } else {
                        let _ = events.send(StickerWindowEvent::TitleChanged { id, title: text });
                    }
                })
                .detach();
            }
        })
        .detach();

        Self {
            store,
            detail,
            sticker_events_tx,
            view,
            last_bounds: None,
            last_bounds_change_at: None,
            error: None,
        }
    }

    fn create_sticker_view(
        detail: &StickerDetail,
        store: &ArcStickerStore,
        window: &mut Window,
        cx: &mut Context<Self>,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    ) -> Box<dyn StickerView> {
        let id = detail.id;
        let color = detail.color;
        let content = detail.content.as_str();
        let store = store.clone();

        match detail.sticker_type {
            StickerType::Timer => Box::new(StickerViewEntity::new(cx.new(|cx| {
                TimerSticker::new(
                    id,
                    color,
                    store,
                    content,
                    window,
                    cx,
                    sticker_events_tx.clone(),
                )
            }))),
            StickerType::Markdown => Box::new(StickerViewEntity::new(cx.new(|cx| {
                MarkdownSticker::new(
                    id,
                    color,
                    store,
                    content,
                    window,
                    cx,
                    sticker_events_tx.clone(),
                )
            }))),
            StickerType::Command => Box::new(StickerViewEntity::new(cx.new(|cx| {
                CommandSticker::new(
                    id,
                    color,
                    store,
                    content,
                    window,
                    cx,
                    sticker_events_tx.clone(),
                )
            }))),
        }
    }

    fn set_error(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        cx.notify();
    }

    fn tick_bounds_state(&mut self, window: &Window, cx: &mut Context<Self>) {
        let current = self.current_bounds(window);

        let changed = self.last_bounds.map(|prev| prev != current).unwrap_or(true);

        if changed {
            self.last_bounds = Some(current);
            self.last_bounds_change_at = Some(Instant::now());
            window.request_animation_frame();
            return;
        }

        if let Some(changed_at) = self.last_bounds_change_at {
            if changed_at.elapsed() >= BOUNDS_SAVE_DEBOUNCE {
                self.last_bounds_change_at = None;
                self.change_bounds(window, cx);
            } else {
                window.request_animation_frame();
            }
        }
    }

    fn try_tick(&mut self, window: &Window, cx: &mut Context<Self>) {
        if window.is_window_hovered() {
            if self.last_bounds.is_none() {
                self.last_bounds = Some(self.current_bounds(window));
            }
            self.tick_bounds_state(window, cx);
        }
    }

    fn current_bounds(&self, window: &Window) -> (i32, i32, i32, i32) {
        let bounds = window.bounds();
        (
            bounds.left().to_f64() as i32,
            bounds.top().to_f64() as i32,
            bounds.size.width.to_f64() as i32,
            bounds.size.height.to_f64() as i32,
        )
    }

    fn change_bounds(&mut self, window: &Window, cx: &mut Context<Self>) {
        let bounds = window.bounds();

        let (left, top, width, height) = (
            bounds.left().to_f64() as i32,
            bounds.top().to_f64() as i32,
            bounds.size.width.to_f64() as i32,
            bounds.size.height.to_f64() as i32,
        );

        if left != self.detail.left
            || top != self.detail.top
            || width != self.detail.width
            || height != self.detail.height
        {
            let id = self.detail.id;
            let store = self.store.clone();
            cx.spawn(async move |this, cx| {
                if let Err(err) = store
                    .update_sticker_bounds(id, left, top, width, height)
                    .await
                {
                    let _ = this.update(cx, |this, cx| {
                        this.set_error(format!("Failed to save window bounds: {err}"), cx);
                    });
                } else {
                    let _ = this.update(cx, |this, _| {
                        this.detail.left = left;
                        this.detail.top = top;
                        this.detail.width = width;
                        this.detail.height = height;
                    });
                }
            })
            .detach();
        }
    }

    fn change_color(&mut self, theme: StickerColor, cx: &mut Context<Self>) {
        self.detail.color = theme;
        self.view.set_color(cx, theme);
        let id = self.detail.id;
        let store = self.store.clone();
        let events = self.sticker_events_tx.clone();
        cx.spawn(async move |entity, cx| {
            if let Err(err) = store
                .update_sticker_color(id, theme.as_str().to_string())
                .await
            {
                let _ = entity.update(cx, |this, cx| {
                    this.set_error(format!("Failed to save color: {err}"), cx);
                });
            } else {
                let _ = events.send(StickerWindowEvent::ColorChanged { id, color: theme });
            }
        })
        .detach();
        cx.notify();
    }

    fn close(&mut self, cx: &mut gpui::App) {
        if !self.view.save_on_close(cx) {
            return;
        }

        let id = self.detail.id;
        let store = self.store.clone();
        let events = self.sticker_events_tx.clone();

        cx.spawn(async move |cx| {
            if let Err(err) = store.update_sticker_state(id, StickerState::Close).await {
                tracing::error!(id, error = %err, "Error saving state on close");
            }

            let _ = events.send(StickerWindowEvent::Closed { id });

            let _ = cx.update(|cx| {
                if !Self::try_close(id, cx) {
                    // Fallback in case not tracked.
                }
            });
        })
        .detach();
    }

    fn header_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .absolute()
            .left_0()
            .top_0()
            .right_0()
            .items_center()
            .gap_2()
            .child(div().size_full().cursor_move()) // Drag handle area
            .child(
                Button::new("close")
                    .bg(rgba(0x000000))
                    .border_0()
                    .cursor_pointer()
                    .icon(IconName::Close)
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            )
            .into_any_element()
    }

    fn footer_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let color_options = h_flex()
            .gap_1()
            .children(StickerColor::ALL.iter().map(|&theme| {
                div()
                    .w(px(16.0))
                    .h(px(16.0))
                    .bg(theme.swatch())
                    .rounded_full()
                    .cursor_pointer()
                    .occlude() // Prevent clicks falling through to resize/drag
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.change_color(theme, cx);
                        }),
                    )
            }));

        h_flex()
            .absolute()
            .justify_end()
            .bottom_0()
            .left_0()
            .right_0()
            .p_2()
            .gap_2()
            .window_control_area(WindowControlArea::Drag)
            .child(color_options)
            .into_any_element()
    }
}

impl Render for StickerWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.try_tick(window, cx);

        window.set_rem_size(cx.theme().font_size);

        v_flex()
            .text_color(cx.theme().foreground)
            .font_family(cx.theme().font_family.clone())
            .relative()
            .size_full()
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |_, window, _| {
                if !window.is_window_active() {
                    window.activate_window();
                }
            })
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.change_bounds(window, cx);
                }),
            )
            .when_some(self.error.as_ref(), |view, msg| {
                view.child(
                    div()
                        .p_2()
                        .child(Alert::error("sticker-error", msg.as_str())),
                )
            })
            .child(self.view.element())
            .when(window.is_window_hovered(), |view| {
                view.child(self.header_view(cx))
            })
            .when(window.is_window_hovered(), |view| {
                view.child(self.footer_view(cx))
            })
    }
}
