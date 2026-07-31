use gpui::{
    Context, Entity, KeyDownEvent, MouseButton, MouseDownEvent, Window, div, prelude::*, px, rgba,
};
use gpui_component::text::TextView;
use gpui_component::{ActiveTheme, Disableable, Sizable, h_flex};
use gpui_component::{
    button::Button,
    input::{Input, InputState},
    v_flex,
};

use crate::model::content::FileStickerContent;
use crate::model::sticker::{StickerColor, StickerDetail, StickerState, StickerType};
use crate::native::components::IconName;
use crate::native::windows::StickerWindowEvent;
use crate::native::windows::sticker::StickerWindow;
use crate::storage::ArcStickerStore;

pub struct MarkdownSticker {
    id: i64,
    color: StickerColor,
    store: ArcStickerStore,
    sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,
    editor: Entity<InputState>,
    editing: bool,
    error: Option<String>,
    converting: bool,
    convert_error: Option<String>,
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
            converting: false,
            convert_error: None,
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

    fn start_convert(&mut self, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        self.converting = true;
        self.convert_error = None;
        cx.notify();

        let content = self.editor.read(cx).value().to_string();
        let initial_name = derive_md_filename(&content);
        let id = self.id;
        let store = self.store.clone();
        let sticker_events_tx = self.sticker_events_tx.clone();

        cx.spawn(async move |entity, cx| {
            // Show the save file dialog asynchronously via rfd.
            let chosen = rfd::AsyncFileDialog::new()
                .set_file_name(format!("{initial_name}.md"))
                .add_filter("Markdown", &["md"])
                .save_file()
                .await
                .map(|h| h.path().to_path_buf());

            let Some(path) = chosen else {
                // User cancelled.
                let _ = entity.update(cx, |this, cx| {
                    this.converting = false;
                    cx.notify();
                });
                return;
            };

            // Write the markdown content to the chosen file.
            if let Err(err) = std::fs::write(&path, &content) {
                let _ = entity.update(cx, |this, cx| {
                    this.converting = false;
                    this.convert_error = Some(format!("Failed to write file: {err}"));
                    cx.notify();
                });
                return;
            }

            // Fetch the current sticker detail so we can copy position/size/color.
            let md_detail = match store.get_sticker(id).await {
                Ok(d) => d,
                Err(err) => {
                    let _ = entity.update(cx, |this, cx| {
                        this.converting = false;
                        this.convert_error = Some(format!("Failed to read sticker: {err:#}"));
                        cx.notify();
                    });
                    return;
                }
            };

            let path_str = path.to_string_lossy().to_string();

            let new_detail = StickerDetail {
                id: 0,
                title: path_str.clone(),
                state: StickerState::Open,
                left: md_detail.left,
                top: md_detail.top,
                width: md_detail.width,
                height: md_detail.height,
                top_most: md_detail.top_most,
                color: md_detail.color,
                sticker_type: StickerType::File,
                content: FileStickerContent::from_sources(&[path_str]).to_json(),
                created_at: 0,
                updated_at: 0,
                display_id: md_detail.display_id,
            };

            // Insert the new file sticker.
            let new_id = match store.insert_sticker(new_detail).await {
                Ok(id) => id,
                Err(err) => {
                    let _ = entity.update(cx, |this, cx| {
                        this.converting = false;
                        this.convert_error =
                            Some(format!("Failed to create file sticker: {err:#}"));
                        cx.notify();
                    });
                    return;
                }
            };

            // Open the new file sticker window.
            if let Err(err) =
                StickerWindow::open_async(cx, sticker_events_tx.clone(), store.clone(), new_id)
                    .await
            {
                tracing::warn!(new_id, error = ?err, "Failed to open new file sticker window");
            }

            // Delete the original markdown sticker from the DB.
            if let Err(err) = store.delete_sticker(id).await {
                tracing::warn!(id, error = ?err, "Failed to delete markdown sticker after convert");
            }

            // Notify the main window to reload its list.
            let _ = sticker_events_tx.send(StickerWindowEvent::Created { id: new_id });

            // Close this markdown window.
            let _ = cx.update(|cx| {
                StickerWindow::try_close(id, cx);
            });
        })
        .detach();
    }
}

/// Derives a filename from the first non-empty line of the markdown content.
fn derive_md_filename(content: &str) -> String {
    let base = content
        .lines()
        .map(|l| l.trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("note");

    // Sanitise: keep only safe filename characters.
    let sanitised: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect();

    let sanitised = sanitised.trim().replace(' ', "_");
    if sanitised.is_empty() {
        "note".to_owned()
    } else {
        sanitised
    }
}

impl super::Sticker for MarkdownSticker {
    fn id(&self) -> i64 {
        self.id
    }

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

    fn footer_extension(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let mut controls = h_flex().flex_1();
        if self.editing {
            controls = controls.child(
                Button::new("save")
                    .label("save (ctrl+s)")
                    .small()
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.save_state(cx);
                    })),
            );
        } else {
            controls = controls.child(
                Button::new("convert-to-file")
                    .icon(IconName::DocumentText)
                    .tooltip("Convert to local file")
                    .disabled(self.converting)
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start_convert(cx);
                    })),
            );
        }
        
        return Some(controls.into_any_element());
    }
}

impl Render for MarkdownSticker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex().size_full().gap_1();

        if self.editing {
            window.set_rem_size(cx.theme().font_size);

            body = body.occlude().child(
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
            );
        } else {
            window.set_rem_size(px(14.0));

            let mut preview_overlay = div()
                .relative()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, e: &MouseDownEvent, _, cx| {
                        if e.click_count >= 2 {
                            this.editing = true;
                            cx.notify();
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
                );

            if let Some(err) = &self.convert_error {
                preview_overlay = preview_overlay.child(
                    div()
                        .occlude()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(gpui::red())
                        .child(err.clone()),
                );
            }

            body = body.child(preview_overlay);
        }

        body
    }
}
