use gpui::{
    Context, Entity, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, Window, div, prelude::*,
    px, rgba,
};
use gpui_component::text::TextView;
use gpui_component::{ActiveTheme, Disableable, h_flex};
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

use super::content_lock::{LockForm, LockedContent};

pub struct MarkdownSticker {
    id: i64,
    color: StickerColor,
    store: ArcStickerStore,
    sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,
    editor: Entity<InputState>,
    editing: bool,
    edit_snapshot: Option<String>,
    error: Option<String>,
    converting: bool,
    convert_error: Option<String>,
    locked_content: Option<LockedContent>,
    content_visible: bool,
    locking: bool,
    lock_busy: bool,
    lock_form: LockForm,
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
        let (locked_content, initial_content, lock_error) = match LockedContent::parse(content) {
            Ok(Some(locked)) => (Some(locked), String::new(), None),
            Ok(None) => (None, content.to_string(), None),
            Err(err) => (None, String::new(), Some(err)),
        };
        let initial_title = locked_content
            .as_ref()
            .map(|locked| locked.title.clone())
            .unwrap_or_else(|| derive_title(&initial_content));
        let editor = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .searchable(true)
                .placeholder("Input text/markdown, ctrl+s to save and preview it")
                .default_value(initial_content)
        });
        let lock_form = LockForm::new(initial_title, "Password to unlock this sticker", window, cx);
        let content_visible = locked_content.is_none() && lock_error.is_none();
        if !content_visible {
            lock_form.focus_password(window, cx);
        }

        let editing = content_visible && content.is_empty();
        Self {
            id,
            color,
            store,
            sticker_events_tx,
            editor,
            editing,
            edit_snapshot: editing.then(String::new),
            error: lock_error,
            converting: false,
            convert_error: None,
            locked_content,
            content_visible,
            locking: false,
            lock_busy: false,
            lock_form,
        }
    }

    fn save_state(&mut self, cx: &mut Context<Self>) -> bool {
        if self.locked_content.is_some() {
            return true;
        }
        let content = self.editor.read(cx).value().to_string();
        let title = derive_title(&content);

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
                this.edit_snapshot = None;
                this.error = None;
                cx.notify();
            });
        })
        .detach();

        true
    }

    fn begin_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing || self.locked_content.is_some() {
            return;
        }
        self.edit_snapshot = Some(self.editor.read(cx).value().to_string());
        self.editing = true;
        self.editor.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(content) = self.edit_snapshot.take() {
            self.editor
                .update(cx, |input, cx| input.set_value(content, window, cx));
        }
        self.editing = false;
        self.error = None;
        cx.notify();
    }

    fn begin_lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = derive_title(self.editor.read(cx).value().as_ref());
        self.lock_form.reset_for_lock(title, window, cx);
        self.locking = true;
        self.error = None;
        cx.notify();
    }

    fn cancel_lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.locking = false;
        self.error = None;
        self.lock_form.clear_passwords(window, cx);
        cx.notify();
    }

    fn cancel_unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        self.lock_form.clear_passwords(window, cx);
        cx.notify();
    }

    fn lock_new_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.lock_busy {
            return;
        }
        let content = self.editor.read(cx).value().to_string();
        let prepared = match self.lock_form.prepare_lock(&content, cx) {
            Ok(prepared) => prepared,
            Err(err) => {
                self.error = Some(err);
                cx.notify();
                return;
            }
        };
        let title = prepared.locked.title.clone();
        let locked = prepared.locked;
        let serialized = prepared.serialized;

        self.lock_busy = true;
        self.error = None;
        let id = self.id;
        let store = self.store.clone();
        let sticker_events_tx = self.sticker_events_tx.clone();
        let entity = cx.entity();
        window
            .spawn(cx, async move |cx| {
                let result = async {
                    store.update_sticker_content(id, serialized).await?;
                    store.update_sticker_title(id, title.clone()).await?;
                    anyhow::Ok(())
                }
                .await;
                let _ = entity.update_in(cx, |this, window, cx| {
                    this.lock_busy = false;
                    match result {
                        Ok(()) => {
                            this.locked_content = Some(locked);
                            this.content_visible = false;
                            this.locking = false;
                            this.editor
                                .update(cx, |input, cx| input.set_value("", window, cx));
                            this.lock_form.clear_passwords(window, cx);
                            this.error = None;
                            let _ = sticker_events_tx
                                .send(StickerWindowEvent::TitleChanged { id, title });
                        }
                        Err(err) => this.error = Some(format!("Failed to lock sticker: {err:#}")),
                    }
                    cx.notify();
                });
            })
            .detach();
    }

    fn unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(locked) = self.locked_content.clone() else {
            return;
        };
        let prepared = match self.lock_form.prepare_unlock(&locked, cx) {
            Ok(prepared) => prepared,
            Err(err) => {
                self.error = Some(err);
                cx.notify();
                return;
            }
        };
        if let Some(updated) = prepared.updated_lock {
            self.locked_content = Some(updated.locked);
            let id = self.id;
            let store = self.store.clone();
            let events = self.sticker_events_tx.clone();
            let title = prepared.title.clone();
            cx.spawn(async move |entity, cx| {
                let result = async {
                    store.update_sticker_content(id, updated.serialized).await?;
                    store.update_sticker_title(id, title.clone()).await?;
                    anyhow::Ok(())
                }
                .await;
                if let Err(err) = result {
                    let _ = entity.update(cx, |this, cx| {
                        this.error = Some(format!(
                            "Unlocked, but failed to save the new title: {err:#}"
                        ));
                        cx.notify();
                    });
                } else {
                    let _ = events.send(StickerWindowEvent::TitleChanged { id, title });
                }
            })
            .detach();
        }
        self.editor.update(cx, |input, cx| {
            input.set_value(prepared.content, window, cx)
        });
        self.content_visible = true;
        self.editing = false;
        self.edit_snapshot = None;
        self.error = None;
        self.lock_form.clear_passwords(window, cx);
        cx.notify();
    }

    fn unlock_forever(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.lock_busy {
            return;
        }
        let Some(locked) = self.locked_content.clone() else {
            return;
        };
        let prepared = match self
            .lock_form
            .prepare_unlock_forever(&locked, derive_title, cx)
        {
            Ok(prepared) => prepared,
            Err(err) => {
                self.error = Some(err);
                cx.notify();
                return;
            }
        };
        let content = prepared.content;
        let title = prepared.title;

        self.lock_busy = true;
        self.error = None;
        let id = self.id;
        let store = self.store.clone();
        let events = self.sticker_events_tx.clone();
        let entity = cx.entity();
        window
            .spawn(cx, async move |cx| {
                let content_result = store.update_sticker_content(id, content.clone()).await;
                let title_result = if content_result.is_ok() {
                    store.update_sticker_title(id, title.clone()).await
                } else {
                    Ok(())
                };
                let _ = entity.update_in(cx, |this, window, cx| {
                    this.lock_busy = false;
                    if let Err(err) = content_result {
                        this.error = Some(format!("Failed to remove sticker lock: {err:#}"));
                        cx.notify();
                        return;
                    }

                    this.locked_content = None;
                    this.content_visible = true;
                    this.editing = false;
                    this.edit_snapshot = None;
                    this.editor
                        .update(cx, |input, cx| input.set_value(content, window, cx));
                    this.lock_form.clear_passwords(window, cx);
                    match title_result {
                        Ok(()) => {
                            this.error = None;
                            let _ = events.send(StickerWindowEvent::TitleChanged { id, title });
                        }
                        Err(err) => {
                            this.error =
                                Some(format!("Lock removed, but failed to save title: {err:#}"));
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
    }

    fn relock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_unlocked_content(window, cx);
    }

    fn hide_unlocked_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.content_visible = false;
        self.editing = false;
        self.edit_snapshot = None;
        self.editor
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.lock_form.clear_passwords(window, cx);
        self.lock_form.focus_password(window, cx);
        self.error = None;
        cx.notify();
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

fn derive_title(content: &str) -> String {
    content
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Private sticker")
        .trim_start_matches('#')
        .trim()
        .to_string()
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

    fn suppress_window_escape(&self) -> bool {
        self.editing || self.locking || !self.content_visible
    }

    fn protected_content_visible(&self) -> bool {
        self.locked_content.is_some() && self.content_visible
    }

    fn relock_protected_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_unlocked_content(window, cx);
    }

    fn handle_lock_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.lock_busy || self.locking {
            return false;
        }
        if self.locked_content.is_some() || !self.content_visible {
            if self.content_visible {
                self.relock(window, cx);
            } else {
                self.lock_form.focus_password(window, cx);
            }
        } else {
            self.begin_lock(window, cx);
        }
        true
    }

    fn header_extension(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let is_protected = self.locked_content.is_some() || !self.content_visible;
        Some(
            h_flex()
                .flex_1()
                .child(div().flex_1())
                .child(
                    Button::new("content-lock")
                        .icon(if is_protected && self.content_visible {
                            IconName::LockOpen
                        } else {
                            IconName::LockClosed
                        })
                        .tooltip(if is_protected && self.content_visible {
                            "Lock sticker now (ctrl+l)"
                        } else if is_protected {
                            "Sticker is locked"
                        } else {
                            "Protect sticker with a password (ctrl+l)"
                        })
                        .disabled(self.lock_busy || self.locking)
                        .bg(rgba(0x000000))
                        .border_0()
                        .cursor_pointer()
                        .occlude()
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.locked_content.is_some() || !this.content_visible {
                                if this.content_visible {
                                    this.relock(window, cx);
                                } else {
                                    this.lock_form.focus_password(window, cx);
                                }
                            } else {
                                this.begin_lock(window, cx);
                            }
                        })),
                )
                .into_any_element(),
        )
    }

    fn footer_extension(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.content_visible || self.locking || self.locked_content.is_some() {
            return None;
        }
        let mut controls = h_flex().flex_1();
        if self.editing {
            controls = controls.child(
                Button::new("save")
                    .label("save (ctrl+s)")
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

        if self.locking {
            window.set_rem_size(px(14.0));
            return self.lock_form.locking_view(
                "markdown",
                "Lock sticker",
                self.error.as_deref(),
                self.lock_busy,
                cx,
                Self::cancel_lock,
                Self::lock_new_content,
            );
        }

        if !self.content_visible {
            window.set_rem_size(px(14.0));
            return self.lock_form.locked_view(
                "markdown",
                self.error.as_deref(),
                self.lock_busy,
                cx,
                Self::cancel_unlock,
                Self::unlock,
                Self::unlock_forever,
            );
        }

        if self.editing {
            window.set_rem_size(cx.theme().font_size);

            body = body.occlude().child(
                div()
                    .size_full()
                    .p_1()
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.modifiers.control
                            && event.keystroke.key.eq_ignore_ascii_case("s")
                        {
                            this.save_state(cx);
                        } else if event.keystroke.key == "escape" {
                            this.cancel_edit(window, cx);
                            cx.stop_propagation();
                            window.prevent_default();
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
                    cx.listener(|this, e: &MouseDownEvent, window, cx| {
                        if e.click_count >= 2 {
                            this.begin_edit(window, cx);
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

        body.into_any_element()
    }
}
