use crate::gpui_component::{
    ActiveTheme, Disableable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{InputEvent, Textarea, TextareaState},
    text::TextView,
    v_flex,
};
use crate::{
    model::content::FileStickerContent,
    model::sticker::{StickerColor, StickerDetail, StickerState, StickerType},
    native::{
        components::IconName,
        windows::{StickerWindowEvent, sticker::StickerWindow, text_password},
    },
    storage::ArcStickerStore,
    text::{TextEntry, TextEntryId, protection},
};
use gpui::{
    Context, Entity, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, Subscription, Window,
    div, prelude::*,
};
use zeroize::Zeroizing;

pub struct MarkdownSticker {
    id: i64,
    color: StickerColor,
    store: ArcStickerStore,
    sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,
    editor: Entity<TextareaState>,
    editing: bool,
    edit_snapshot: Option<Zeroizing<String>>,
    error: Option<String>,
    converting: bool,
    convert_error: Option<String>,
    entry: Option<TextEntry>,
    encrypted: bool,
    content_visible: bool,
    busy: bool,
    _changes: Subscription,
}
impl Drop for MarkdownSticker {
    fn drop(&mut self) {
        text_password::set_dirty(self.id, false);
    }
}
impl MarkdownSticker {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: i64,
        color: StickerColor,
        store: ArcStickerStore,
        content: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,
        edit: bool,
    ) -> Self {
        let encrypted = protection::is_encrypted(content);
        let unsupported = protection::is_unsupported(content);
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .searchable(true)
                .placeholder("Text / Markdown")
                .default_value(if encrypted || unsupported {
                    ""
                } else {
                    content
                })
        });
        let changes = cx.subscribe(&editor, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && this.encrypted && this.content_visible {
                text_password::set_dirty(this.id, true);
            }
            cx.notify();
        });
        let mut this = Self {
            id,
            color,
            store,
            sticker_events_tx,
            editor,
            editing: !encrypted && !unsupported && (edit || content.is_empty()),
            edit_snapshot: None,
            error: unsupported.then(|| {
                "This legacy lock format is unsupported. No recovery or migration is provided."
                    .into()
            }),
            converting: false,
            convert_error: None,
            entry: None,
            encrypted,
            content_visible: !encrypted && !unsupported,
            busy: false,
            _changes: changes,
        };
        if this.editing {
            this.edit_snapshot = Some(Zeroizing::new(content.to_owned()));
            this.editor.focus_handle(cx).focus(window, cx);
        }
        let repo = crate::native::windows::clipboard::text_repository();
        if let Some(repo) = repo {
            cx.spawn(async move |entity, cx| {
                let entry = repo.get(TextEntryId::Sticker(id)).await;
                let _ = entity.update(cx, |this, cx| {
                    match entry {
                        Ok(entry) => this.entry = Some(entry),
                        Err(_) => {
                            this.error = Some("Text is unavailable. Reopen the sticker.".into())
                        }
                    };
                    cx.notify();
                });
            })
            .detach();
        }
        this
    }
    fn reset_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor = cx.new(|cx| TextareaState::new(window, cx).searchable(true));
        self._changes = cx.subscribe(&self.editor, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && this.encrypted && this.content_visible {
                text_password::set_dirty(this.id, true);
            }
            cx.notify();
        });
    }

    pub(crate) fn begin_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || text_password::is_rotating() {
            return;
        }
        if !self.content_visible {
            self.request_unlock(window, cx);
            return;
        }
        self.edit_snapshot = Some(Zeroizing::new(self.editor.read(cx).value().to_string()));
        self.editing = true;
        self.editor.focus_handle(cx).focus(window, cx);
        cx.notify();
    }
    fn request_unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.encrypted {
            return;
        }
        let entity = cx.entity().downgrade();
        text_password::unlock(
            window,
            cx,
            Box::new(move |password, window, cx| {
                if let Some(entity) = entity.upgrade() {
                    entity.update(cx, |this, cx| this.authorize(password, window, cx));
                }
            }),
        );
    }
    fn authorize(
        &mut self,
        password: Zeroizing<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = crate::native::windows::clipboard::text_repository() else {
            return;
        };
        let id = self.id;
        self.busy = true;
        cx.notify();
        let entity = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        futures::executor::block_on(async {
                            let entry = repo.get(TextEntryId::Sticker(id)).await?;
                            let text = repo.resolve(&entry, Some(&password)).await?;
                            anyhow::Ok((entry, text))
                        })
                    })
                    .await;
                if let Some(entity) = entity.upgrade() {
                    let _ = entity.update_in(cx, |this, window, cx| {
                        this.busy = false;
                        match result {
                            Ok((entry, text)) => {
                                this.entry = Some(entry);
                                this.editor.update(cx, |input, cx| {
                                    input.set_value(text.to_string(), window, cx)
                                });
                                this.edit_snapshot = Some(text);
                                this.content_visible = true;
                                this.editing = true;
                                this.error = None;
                                text_password::set_dirty(this.id, false);
                            }
                            Err(_) => {
                                this.error = Some(
                                    "Could not unlock text. Check the password and try again."
                                        .into(),
                                )
                            }
                        };
                        cx.notify();
                    });
                }
            })
            .detach();
    }
    fn save_state(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.content_visible || !self.editing {
            return true;
        }
        if self.busy || text_password::is_rotating() {
            return false;
        }
        let Some(repo) = crate::native::windows::clipboard::text_repository() else {
            self.error = Some("Text storage unavailable.".into());
            cx.notify();
            return false;
        };
        let Some(expected) = self.entry.clone() else {
            self.error = Some("Text is still loading. Try again.".into());
            cx.notify();
            return false;
        };
        let text = Zeroizing::new(self.editor.read(cx).value().to_string());
        self.busy = true;
        cx.notify();
        cx.spawn(async move|entity,cx|{let result=repo.save_text(&expected,&text).await;
            let fresh=if result.is_ok(){repo.get(expected.id()).await.ok()}else{None};
            let _=entity.update(cx,|this,cx|{this.busy=false;match result{Ok(())=>{this.entry=fresh;this.editing=false;this.edit_snapshot=None;this.error=None;text_password::set_dirty(this.id,false);let _=this.sticker_events_tx.send(StickerWindowEvent::Created{id:this.id});},Err(_)=>this.error=Some("Text changed or could not be saved. Keep your draft and reopen the current text.".into())};cx.notify();});
        }).detach();
        false
    }
    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(snapshot) = self.edit_snapshot.take() {
            self.editor.update(cx, |input, cx| {
                input.set_value(snapshot.to_string(), window, cx)
            });
        }
        self.editing = false;
        text_password::set_dirty(self.id, false);
        cx.notify();
    }
    fn hide_unlocked_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.encrypted || self.busy {
            return;
        }
        let dirty = self.editing
            && self
                .edit_snapshot
                .as_ref()
                .is_some_and(|s| s.as_str() != self.editor.read(cx).value().as_ref());
        if dirty {
            self.error = Some("Save or discard your encrypted draft before locking.".into());
            cx.notify();
            return;
        }
        self.reset_editor(window, cx);
        self.edit_snapshot = None;
        self.content_visible = false;
        self.editing = false;
        text_password::set_dirty(self.id, false);
        cx.notify();
    }
    fn lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if !self.encrypted && !self.editing {
            let weak = cx.weak_entity();
            let title = self.entry.as_ref().map(|entry| entry.title()).unwrap_or("");
            crate::native::windows::text_title::prompt(
                window,
                cx,
                true,
                title,
                Box::new(move |title, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |this, cx| this.lock_titled(Some(title), window, cx));
                    }
                }),
            );
            return;
        }
        self.lock_titled(None, window, cx);
    }
    fn lock_titled(&mut self, title: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.encrypted {
            if self.content_visible {
                self.hide_unlocked_content(window, cx)
            } else {
                self.request_unlock(window, cx)
            };
            return;
        }
        if self.editing {
            self.error = Some("Save your text before locking it.".into());
            cx.notify();
            return;
        }
        let Some(repo) = crate::native::windows::clipboard::text_repository() else {
            return;
        };
        let Some(entry) = self.entry.clone() else {
            return;
        };
        let entity = cx.entity().downgrade();
        self.busy = true;
        cx.notify();
        window
            .spawn(cx, async move |cx| {
                let probe = repo.clone();
                let configured = cx
                    .background_executor()
                    .spawn(async move { futures::executor::block_on(probe.master_key()) })
                    .await;
                if !matches!(configured, Ok(Some(_))) {
                    if let Some(entity) = entity.upgrade() {
                        let _ = entity.update_in(cx, |this, window, cx| {
                            this.busy = false;
                            text_password::settings(window, cx, repo);
                            cx.notify();
                        });
                    }
                    return;
                }
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        futures::executor::block_on(repo.set_encryption_titled(
                            &entry,
                            true,
                            None,
                            title.as_deref(),
                        ))
                    })
                    .await;
                if let Some(entity) = entity.upgrade() {
                    let _ = entity.update_in(cx, |this, window, cx| {
                        this.busy = false;
                        match result {
                            Ok(()) => {
                                this.encrypted = true;
                                this.content_visible = false;
                                this.edit_snapshot = None;
                                this.reset_editor(window, cx);
                                this.error = None;
                                let _ = this
                                    .sticker_events_tx
                                    .send(StickerWindowEvent::Created { id: this.id });
                            }
                            Err(_) => {
                                this.error =
                                    Some("Could not lock text. Reload and try again.".into())
                            }
                        };
                        cx.notify();
                    });
                }
            })
            .detach();
    }
    fn remove_encryption(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.editing || text_password::is_rotating() {
            return;
        }
        let entity = cx.entity().downgrade();
        text_password::unlock(
            window,
            cx,
            Box::new(move |password, window, cx| {
                if let Some(entity) = entity.upgrade() {
                    entity.update(cx, |this, cx| {
                        let Some(repo) = crate::native::windows::clipboard::text_repository()
                        else {
                            return;
                        };
                        let id = this.id;
                        this.busy = true;
                        let weak = cx.entity().downgrade();
                        window
                            .spawn(cx, async move |cx| {
                                let result = cx
                                    .background_executor()
                                    .spawn(async move {
                                        futures::executor::block_on(async {
                                            let entry = repo.get(TextEntryId::Sticker(id)).await?;
                                            repo.set_encryption(&entry, false, Some(&password))
                                                .await?;
                                            let current = repo.get(entry.id()).await?;
                                            let text = repo.resolve(&current, None).await?;
                                            anyhow::Ok((current, text))
                                        })
                                    })
                                    .await;
                                if let Some(entity) = weak.upgrade() {
                                    let _ = entity.update_in(cx, |this, window, cx| {
                                        this.busy = false;
                                        match result {
                                            Ok((entry, text)) => {
                                                this.entry = Some(entry);
                                                this.encrypted = false;
                                                this.content_visible = true;
                                                this.editor.update(cx, |input, cx| {
                                                    input.set_value(text.to_string(), window, cx)
                                                });
                                                this.editing = false;
                                                this.edit_snapshot = None;
                                                text_password::set_dirty(id, false);
                                                let _ = this
                                                    .sticker_events_tx
                                                    .send(StickerWindowEvent::Created { id });
                                            }
                                            Err(_) => {
                                                this.error = Some(
                                                    "Could not remove encryption; nothing changed."
                                                        .into(),
                                                )
                                            }
                                        };
                                        cx.notify();
                                    });
                                }
                            })
                            .detach();
                        cx.notify();
                    });
                }
            }),
        );
    }
    fn start_convert(&mut self, cx: &mut Context<Self>) {
        if self.encrypted {
            return;
        }
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
                display_uuid: md_detail.display_uuid.clone(),
                virtual_desktop_id: md_detail.virtual_desktop_id.clone(),
                native_left: None,
                native_top: None,
                native_width: None,
                native_height: None,
                preferred_display_uuid: None,
                placements: Vec::new(),
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
            cx.update(|cx| {
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
    fn suppress_window_escape(&self) -> bool {
        self.editing || self.busy
    }
    fn protected_content_visible(&self) -> bool {
        self.encrypted && self.content_visible
    }
    fn relock_protected_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_unlocked_content(window, cx);
    }
    fn begin_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        MarkdownSticker::begin_edit(self, window, cx);
    }
    fn handle_lock_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.lock(window, cx);
        true
    }
    fn header_extension(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        Some(
            h_flex()
                .flex_1()
                .child(div().flex_1())
                .child(
                    Button::new("text-lock")
                        .icon(if self.encrypted && self.content_visible {
                            IconName::LockOpen
                        } else {
                            IconName::LockClosed
                        })
                        .tooltip("Lock / unlock text (Ctrl+L)")
                        .ghost()
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.lock(window, cx))),
                )
                .into_any_element(),
        )
    }
    fn footer_extension(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.content_visible {
            return None;
        }
        let mut controls = h_flex().gap_2();
        if self.editing {
            controls = controls
                .child(
                    Button::new("text-save")
                        .label("Save")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.save_state(cx);
                        })),
                )
                .child(
                    Button::new("text-discard")
                        .label("Discard edits")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.cancel_edit(window, cx))),
                );
        } else if self.encrypted {
            controls = controls.child(
                Button::new("text-unprotect")
                    .label("Remove encryption…")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.remove_encryption(window, cx)),
                    ),
            );
        } else {
            controls = controls.child(
                Button::new("convert-to-file")
                    .icon(IconName::DocumentText)
                    .tooltip("Convert to local file…")
                    .disabled(self.converting)
                    .on_click(cx.listener(|this, _, _, cx| this.start_convert(cx))),
            );
        }
        Some(controls.into_any_element())
    }
}
impl Render for MarkdownSticker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex().size_full().gap_2();
        if let Some(error) = &self.error {
            body = body.child(div().text_color(cx.theme().danger).child(error.clone()));
        }
        if !self.content_visible {
            return body
                .p_4()
                .child(
                    Button::new("unlock-text")
                        .label("Unlock text…")
                        .disabled(self.busy || !self.encrypted)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.request_unlock(window, cx)),
                        ),
                )
                .into_any_element();
        }
        if self.editing {
            body = body.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if (event.keystroke.modifiers.control || event.keystroke.modifiers.platform)
                            && event.keystroke.key.eq_ignore_ascii_case("s")
                        {
                            this.save_state(cx);
                            cx.stop_propagation();
                        } else if event.keystroke.key == "escape" {
                            this.cancel_edit(window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        Textarea::new(&self.editor)
                            .size_full()
                            .bordered(false)
                            .readonly(self.busy || text_password::is_rotating()),
                    ),
            );
        } else {
            body = body.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            if event.click_count >= 2 {
                                this.begin_edit(window, cx);
                            }
                        }),
                    )
                    .child(TextView::markdown(
                        ("markdown-text", self.id as u64),
                        self.editor.read(cx).value().to_string(),
                    )),
            );
        }
        body.into_any_element()
    }
}
