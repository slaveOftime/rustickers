use gpui::{
    Context, Entity, KeyDownEvent, MouseButton, MouseDownEvent, Rgba, Window, WindowControlArea,
    div, prelude::*, px, rgba,
};
use gpui_component::text::TextView;
use gpui_component::{ActiveTheme, Sizable, h_flex};
use gpui_component::{
    button::Button,
    input::{Input, InputState},
    v_flex,
};

use crate::model::sticker::StickerColor;
use crate::storage::ArcStickerStore;
use crate::windows::StickerWindowEvent;

pub struct MarkdownSticker {
    id: i64,
    color: StickerColor,
    store: ArcStickerStore,
    sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,
    editor: Entity<InputState>,
    editing: bool,
    error: Option<String>,
}

impl MarkdownSticker {
    pub fn new(
        id: i64,
        color: StickerColor,
        store: ArcStickerStore,
        content: &str,
        window: &mut Window,
        cx: &mut Context<MarkdownSticker>,
        sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,
    ) -> Self {
        let editor = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .searchable(true)
                .placeholder("Input text/markdown, ctrl+s to save and preview it")
                .default_value(content.to_string())
        });

        Self {
            id,
            color,
            store,
            sticker_events_tx,
            editor,
            editing: content.is_empty(),
            error: None,
        }
    }

    fn save_state(&mut self, cx: &mut Context<Self>) -> bool {
        let content = self.editor.read(cx).value().to_string();

        let title = content
            .lines()
            .filter(|x| !x.is_empty())
            .next()
            .unwrap_or("")
            .to_string();

        let id = self.id;
        let store = self.store.clone();
        let sticker_events_tx = self.sticker_events_tx.clone();

        cx.spawn(async move |entity, cx| {
            if let Err(err) = store.update_sticker_title(id, title.clone()).await {
                let _ = entity.update(cx, |this, cx| {
                    this.error = Some(format!("{err:#}"));
                    cx.notify();
                });
                return;
            }

            if let Err(err) = sticker_events_tx.send(StickerWindowEvent::TitleChanged { id, title })
            {
                tracing::warn!(
                    id,
                    error = %err,
                    "Failed to send title changed event for markdown sticker"
                );
            }

            if let Err(err) = store.update_sticker_content(id, content).await {
                let _ = entity.update(cx, |this, cx| {
                    this.error = Some(format!("{err:#}"));
                    cx.notify();
                });
                return;
            }

            let _ = entity.update(cx, |this, cx| {
                this.editing = false;
                this.error = None;
                cx.notify();
            });
        })
        .detach();

        true
    }
}

impl super::Sticker for MarkdownSticker {
    fn save_on_close(&mut self, cx: &mut Context<Self>) -> bool {
        self.save_state(cx)
    }

    fn min_window_size() -> gpui::Size<i32> {
        gpui::size(200, 100)
    }

    fn default_window_size() -> gpui::Size<i32> {
        gpui::size(400, 300)
    }

    fn set_color(&mut self, color: StickerColor) {
        self.color = color;
    }
}

impl Render for MarkdownSticker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex().size_full().gap_1().bg(Rgba {
            a: 0.85,
            ..self.color.bg()
        });

        if self.editing {
            window.set_rem_size(cx.theme().font_size);

            body = body
                .child(
                    div()
                        .size_full()
                        .p_1()
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                            if event.keystroke.modifiers.control
                                && event.keystroke.key.eq_ignore_ascii_case("s")
                            {
                                this.save_state(cx);
                            }
                        }))
                        .child(
                            Input::new(&self.editor)
                                .size_full()
                                .bordered(false)
                                .bg(rgba(0x000000)),
                        ),
                )
                .child(
                    h_flex().child(Button::new("save").label("save (ctrl+s)").small().on_click(
                        cx.listener(|s, _, _, cx| {
                            s.save_state(cx);
                        }),
                    )),
                );
        } else {
            window.set_rem_size(px(14.0));
            body = body.child(
                div()
                    .size_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|s, e: &MouseDownEvent, _, _| {
                            if e.click_count >= 2 {
                                s.editing = true;
                            }
                        }),
                    )
                    .child(
                        TextView::markdown("markdown-preview", self.editor.read(cx).value())
                            .py_1()
                            .px_2()
                            .size_full()
                            .selectable(true)
                            .scrollable(true),
                    )
                    .child(
                        div()
                            .occlude()
                            .absolute()
                            .left_0()
                            .top_0()
                            .right_0()
                            .h_5()
                            .window_control_area(WindowControlArea::Drag),
                    ),
            );
        }

        body
    }
}
