//! What a sticker window draws: the shared chrome around whichever sticker view it hosts.

use std::sync::mpsc;

use gpui::{
    AnyElement, Context, IntoElement, MouseButton, Render, Rgba, Window, WindowControlArea, div,
    prelude::*, px, rgba,
};
use gpui_component::{
    ActiveTheme,
    alert::Alert,
    button::Button,
    h_flex,
    menu::{DropdownMenu, PopupMenuItem},
    v_flex,
};

use crate::model::sticker::{StickerColor, StickerDetail, StickerState, StickerType};
use crate::native::components::{
    IconName,
    stickers::{
        StickerView, StickerViewEntity,
        command::{CommandSticker, CommandStickerInit, CommandStickerWindowRequest},
        file::FileSticker,
        markdown::MarkdownSticker,
        paint::PaintSticker,
        timer::TimerSticker,
    },
};
use crate::native::windows::StickerWindowEvent;
use crate::storage::ArcStickerStore;

use super::open::{OpenOptions, default_window_size};
use super::{StickerWindow, platform};

impl Render for StickerWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.try_tick(window, cx);

        window.set_rem_size(cx.theme().font_size);

        v_flex()
            .text_color(cx.theme().foreground)
            .font_family(cx.theme().font_family.clone())
            .relative()
            .size_full()
            .on_mouse_down(MouseButton::Left, |event, window, cx| {
                if !window.is_window_active() {
                    window.activate_window();
                }
                if event.click_count >= 2 {
                    cx.stop_propagation();
                    window.prevent_default();
                }
            })
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.change_bounds(window, cx);
                }),
            )
            .when(self.view.use_default_bg(cx), |view| {
                view.bg(Rgba {
                    a: 0.85,
                    ..self.detail.color.bg()
                })
            })
            .when_some(self.error.as_ref(), |view, msg| {
                view.child(
                    div()
                        .p_2()
                        .child(Alert::error("sticker-error", msg.as_str())),
                )
            })
            .child(self.view.element())
            .when(window.is_window_active(), |view| {
                view.child(self.header_view(cx)).child(self.footer_view(cx))
            })
    }
}

impl StickerWindow {
    /// Build the sticker-type specific view this window hosts.
    pub(super) fn create_sticker_view(
        detail: &StickerDetail,
        store: &ArcStickerStore,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        options: &OpenOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Box<dyn StickerView> {
        let id = detail.id;
        let color = detail.color;
        let content = detail.content.as_str();
        let store = store.clone();

        match detail.sticker_type {
            StickerType::Timer => Box::new(StickerViewEntity::new(cx.new(|cx| {
                TimerSticker::new(id, color, store, content, window, cx, sticker_events_tx)
            }))),
            StickerType::Markdown => Box::new(StickerViewEntity::new(cx.new(|cx| {
                MarkdownSticker::new(id, color, store, content, window, cx, sticker_events_tx)
            }))),
            StickerType::Command => {
                let init = CommandStickerInit {
                    id,
                    color,
                    store,
                    title: detail.title.clone(),
                    content: detail.content.clone(),
                    sticker_events_tx,
                    selection: options.selection.clone(),
                    open_in_settings: options.open_in_settings,
                    window_hidden: options.hidden,
                };
                let entity = cx.new(|cx| CommandSticker::new(init, window, cx));

                cx.subscribe_in(
                    &entity,
                    window,
                    |this, _, event: &CommandStickerWindowRequest, window, cx| match event {
                        CommandStickerWindowRequest::Close => this.close(window, cx),
                        CommandStickerWindowRequest::Show => {
                            platform::refocus_window(window, this.detail.top_most);
                            window.refresh();
                            // GPUI holds back the placement of a window opened hidden and applies
                            // it here, computed from the primary monitor's scale factor. Claim the
                            // placement back once the window is really on screen.
                            this.rearm_restore(cx);
                            window.activate_window();
                        }
                    },
                )
                .detach();

                Box::new(StickerViewEntity::new(entity))
            }
            StickerType::Paint => {
                Box::new(StickerViewEntity::new(cx.new(|_| {
                    PaintSticker::new(id, color, store, content, sticker_events_tx)
                })))
            }
            StickerType::File => Box::new(StickerViewEntity::new(cx.new(|cx| {
                FileSticker::new(id, color, store, content, window, cx, sticker_events_tx)
            }))),
        }
    }

    fn header_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let extension = self.view.header_extension(cx);

        h_flex()
            .absolute()
            .left_0()
            .top_0()
            .right_0()
            .items_center()
            .cursor_grab()
            .window_control_area(WindowControlArea::Drag)
            .occlude()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when_some(extension, |view, extension| view.child(extension)),
            )
            .child(self.create_button(cx))
            .child(
                Button::new("close")
                    .bg(rgba(0x000000))
                    .border_0()
                    .cursor_pointer()
                    .icon(IconName::Close)
                    .occlude()
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
            )
            .into_any_element()
    }

    fn footer_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let is_absoulte = self.view.is_footer_absoute(cx);
        let extension = self.view.footer_extension(cx);
        let bg_color = Rgba {
            a: 0.85,
            ..self.detail.color.bg()
        };
        let color_options = h_flex()
            .p_2()
            .gap_1()
            .children(StickerColor::ALL.iter().map(|&theme| {
                div()
                    .w(px(16.0))
                    .h(px(16.0))
                    .bg(theme.swatch())
                    .rounded_full()
                    .cursor_pointer()
                    .window_control_area(WindowControlArea::Drag)
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.change_color(theme, cx);
                        }),
                    )
            }));

        h_flex()
            .when(is_absoulte, |v| v.absolute().bottom_0().left_0().right_0())
            .when(!is_absoulte, |v| v.bg(bg_color))
            .justify_end()
            .gap_2()
            .items_center()
            .occlude()
            .cursor_grab()
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when_some(extension, |view, extension| view.child(extension)),
            )
            .when(!self.view.disable_color_picker(cx), move |v| {
                v.child(color_options)
            })
            .into_any_element()
    }

    fn create_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let root_entity = cx.entity();
        Button::new("create")
            .border_0()
            .bg(rgba(0x00000000))
            .icon(IconName::Plus)
            .opacity(0.8)
            .dropdown_menu(move |mut menu, window, _| {
                for (id, sticker_type) in [
                    ("text", StickerType::Markdown),
                    ("timer", StickerType::Timer),
                    ("command", StickerType::Command),
                    ("paint", StickerType::Paint),
                ] {
                    menu = menu.item(
                        PopupMenuItem::new(id)
                            .icon(sticker_type_icon(sticker_type))
                            .on_click(window.listener_for(&root_entity, move |this, _, _, cx| {
                                this.create_sticker(sticker_type, cx);
                            })),
                    );
                }
                menu
            })
            .into_any_element()
    }

    /// Save a brand new sticker of the given type and open it in its own window.
    fn create_sticker(&mut self, sticker_type: StickerType, cx: &mut Context<Self>) {
        let size = default_window_size(sticker_type);
        let detail = StickerDetail {
            id: 0,
            title: format!("New {} Sticker", sticker_type_label(sticker_type)),
            content: String::new(),
            color: StickerColor::Yellow,
            sticker_type,
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
        let sticker_events_tx = self.sticker_events_tx.clone();
        cx.spawn(
            async move |entity, cx| match store.insert_sticker(detail).await {
                Ok(id) => {
                    let _ = sticker_events_tx.send(StickerWindowEvent::Created { id });

                    if let Err(err) =
                        StickerWindow::open_async(cx, sticker_events_tx.clone(), store.clone(), id)
                            .await
                    {
                        let _ = entity.update(cx, |this, cx| {
                            this.set_error(format!("Failed to open sticker window: {err:#}"), cx);
                        });
                    }
                }
                Err(err) => {
                    let _ = entity.update(cx, |this, cx| {
                        this.set_error(format!("Failed to create sticker: {err:#}"), cx);
                    });
                }
            },
        )
        .detach();
    }
}

fn sticker_type_icon(sticker_type: StickerType) -> IconName {
    match sticker_type {
        StickerType::Markdown | StickerType::File => IconName::DocumentText,
        StickerType::Command => IconName::Command,
        StickerType::Timer => IconName::Bell,
        StickerType::Paint => IconName::Paint,
    }
}

fn sticker_type_label(sticker_type: StickerType) -> &'static str {
    match sticker_type {
        StickerType::Markdown => "Text",
        StickerType::Command => "Command",
        StickerType::Timer => "Timer",
        StickerType::Paint => "Paint",
        StickerType::File => "File",
    }
}
