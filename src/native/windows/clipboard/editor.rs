//! In-history modal editor with atomic saves and scoped protected authorization.
use super::{ClipboardChooserWidget, protected};
use crate::clipboard::{items::ClipboardItem, storage::ClipboardHistoryStore};

use crate::gpui_component::{
    ActiveTheme, Disableable, Sizable, WindowExt,
    alert::Alert,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState, Textarea, TextareaState},
    v_flex,
};
use gpui::{
    App, Context, Entity, Focusable, IntoElement, Render, Subscription, WeakEntity, Window, div,
    prelude::*, px,
};
use std::sync::Arc;

pub(super) fn open(
    window: &mut Window,
    cx: &mut App,
    store: Arc<dyn ClipboardHistoryStore>,
    initial: ClipboardItem,
    parent: WeakEntity<ClipboardChooserWidget>,
    authorized: Option<(
        zeroize::Zeroizing<String>,
        Arc<crate::clipboard::vault_controller::VaultController>,
    )>,
    prefilled_label: Option<String>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !protected(&initial) || authorized.is_some(),
        "Encrypted editing requires authorization"
    );
    let view = cx.new(|cx| {
        ClipboardEditor::new(
            window,
            cx,
            store,
            initial,
            parent,
            authorized,
            prefilled_label,
        )
    });
    let focus = view.read(cx).focus_handle(cx);
    window.open_dialog(cx, move |dialog, window, cx| {
        dialog
            .title(
                h_flex()
                    .w_full()
                    .items_center()
                    .child(div().flex_1().child("Edit clipboard text"))
                    .child(
                        Button::new("close-clipboard-editor")
                            .icon(crate::native::components::IconName::Close)
                            .ghost()
                            .small()
                            .tooltip("Close · Esc")
                            .disabled(view.read(cx).saving)
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    view.update(cx, |this, cx| this.cancel(window, cx));
                                }
                            }),
                    ),
            )
            .w(window.viewport_size().width.min(px(500.0)) - px(32.0))
            .text_sm()
            .overlay_closable(false)
            .keyboard(false)
            .close_button(false)
            .child(view.clone())
    });
    focus.focus(window, cx);
    Ok(())
}
struct ClipboardEditor {
    store: Arc<dyn ClipboardHistoryStore>,
    initial: ClipboardItem,
    content: Entity<TextareaState>,
    /// Optional public alias. Pre-filled from `initial.public_label` (or from the
    /// caller when the protected-edit password dialog pre-fills with a fresh value).
    /// Empty/whitespace maps to `public_label = NULL` at save time.
    label: Entity<InputState>,
    original_label: Option<String>,
    parent: WeakEntity<ClipboardChooserWidget>,
    original_plain: zeroize::Zeroizing<String>,
    vault: Option<Arc<crate::clipboard::vault_controller::VaultController>>,
    saving: bool,
    confirm_discard: bool,
    error: Option<String>,
    _keys: Subscription,
}
impl ClipboardEditor {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        store: Arc<dyn ClipboardHistoryStore>,
        initial: ClipboardItem,
        parent: WeakEntity<ClipboardChooserWidget>,
        authorized: Option<(
            zeroize::Zeroizing<String>,
            Arc<crate::clipboard::vault_controller::VaultController>,
        )>,
        prefilled_label: Option<String>,
    ) -> Self {
        let (original_plain, vault) = match authorized {
            Some((text, vault)) => (text, Some(vault)),
            None => (zeroize::Zeroizing::new(initial.content.clone()), None),
        };
        // Pre-fill priority:
        //   `Some(s)` -- caller is being explicit; empty/whitespace-only values
        //     shorten to `""` and through the save pipeline null out the column.
        //   `None`   -- the password dialog saw no edit; fall back to the row's
        //     stored alias so an idle unlock doesn't change the alias.
        let initial_label = initial
            .public_label
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_owned();
        let start_with_label = match prefilled_label.as_deref() {
            Some(p) => p.trim().to_owned(),
            None => initial_label.clone(),
        };
        let label_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Public label (optional)")
                .default_value(start_with_label.clone())
        });
        let content = cx.new(|cx| {
            let mut text = TextareaState::new(window, cx);
            text.set_value(original_plain.to_string(), window, cx);
            text
        });
        let own_id = window.window_handle().window_id();
        let entity = cx.weak_entity();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle().window_id() != own_id || !window.is_window_active() {
                return;
            }
            let primary = if cfg!(target_os = "macos") {
                event.keystroke.modifiers.platform || event.keystroke.modifiers.control
            } else {
                event.keystroke.modifiers.control
            };
            let Some(entity) = entity.upgrade() else {
                return;
            };
            let handled = match event.keystroke.key.as_str() {
                "s" if primary => {
                    entity.update(cx, |this, cx| this.save(window, cx));
                    true
                }
                "escape" => {
                    entity.update(cx, |this, cx| this.cancel(window, cx));
                    true
                }
                _ => false,
            };
            if handled {
                cx.stop_propagation();
                window.prevent_default();
            }
        });
        content.update(cx, |text, cx| text.focus(window, cx));
        let protected_editor = vault.is_some();
        if protected_editor {
            cx.spawn_in(window, async move |entity, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(300))
                    .await;
                let _ = entity.update_in(cx, |this, window, cx| this.close(window, cx));
            })
            .detach();
        }
        Self {
            store,
            initial,
            content,
            label: label_state,
            original_label: Some(start_with_label),
            parent,
            original_plain,
            vault,
            saving: false,
            confirm_discard: false,
            error: None,
            _keys: keys,
        }
    }
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.content
            .update(cx, |text, cx| text.set_value("", window, cx));
        if let Some(vault) = &self.vault {
            vault.lock();
        }
        self.original_plain.clear();
        window.close_dialog(cx);
        let _ = self.parent.update(cx, |parent, cx| {
            parent.editor_open = false;
            parent.query.update(cx, |q, cx| q.focus(window, cx));
            cx.notify();
        });
    }
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let content_changed =
            self.content.read(cx).value().as_str() != self.original_plain.as_str();
        let label_changed =
            self.label.read(cx).value().as_str() != self.original_label.as_deref().unwrap_or("");
        if (content_changed || label_changed) && !self.confirm_discard {
            self.confirm_discard = true;
            self.error = Some(
                "Unsaved changes. Press Esc again or Cancel to discard; Save keeps them.".into(),
            );
            cx.notify();
            return;
        }
        self.close(window, cx);
    }
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let text = zeroize::Zeroizing::new(self.content.read(cx).value().to_string());
        // Read the label before we commit to saving so we can diff it against the
        // pre-fill on this same open -- "label-only changes" still go through the
        // store so the user's edit isn't lost the moment they rescue a stuck alias.
        let raw_label = self.label.read(cx).value();
        let label_trimmed = raw_label.trim().to_string();
        let label_changed = Some(label_trimmed.as_str()) != self.original_label.as_deref();
        let content_changed = text.as_str() != self.original_plain.as_str();
        if !content_changed && !label_changed {
            self.close(window, cx);
            return;
        }
        // Move the trimmed label into the spawned task so its lifetime is not
        // tied to the editor borrow; `Some("")` and `Some("   ") reduce to NULL
        // inside the storage layer.
        let label_for_storage: Option<String> = if label_changed {
            Some(label_trimmed)
        } else {
            None
        };
        self.saving = true;
        self.error = None;
        cx.notify();
        let store = self.store.clone();
        let initial = self.initial.clone();
        let parent = self.parent.clone();
        let vault = self.vault.clone();
        cx.spawn_in(window, async move |entity, cx| {
            // Re-borrow the storage path with the label value held by the task.
            let label_ref: Option<&str> = label_for_storage.as_deref();
            let result = async {
                if let Some(vault) = vault {
                    let sealed = vault.seal_for_storage(text.as_bytes())?.ok_or_else(|| anyhow::anyhow!("Authorization expired; reopen to unlock"))?;
                    store
                        .compare_exchange_protected_content_bound(
                            initial.id,
                            &initial.content,
                            &sealed.ciphertext,
                            text.len() as i64,
                            crate::utils::time::now_unix_millis(),
                            &sealed.vault_envelope,
                            label_ref,
                        )
                        .await
                } else {
                    // Compare-and-swap is atomic with edited retention and rejects protection/deletion races.
                    store
                        .compare_exchange_content(
                            initial.id,
                            &initial.content,
                            &text,
                            text.len() as i64,
                            crate::utils::time::now_unix_millis(),
                            label_ref,
                        )
                        .await
                }
            }.await;
            let _ = entity.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(()) => {
                        this.close(window, cx);
                        let _ = parent.update(cx, |this, cx| this.refresh(window, cx));                    }
                    Err(_) => { this.error = Some("Save failed or the item changed/deleted. Your draft is still here; retry or cancel.".into()); cx.notify(); }
                }
            });
        }).detach();
    }
}
impl Focusable for ClipboardEditor {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.content.read(cx).focus_handle(cx)
    }
}
impl Render for ClipboardEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_2()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .h(px(200.0))
                    .min_h_0()
                    .child(Textarea::new(&self.content).size_full()),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(Input::new(&self.label).w_full().small())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                "Public alias shown in history without unlocking. Leave empty to clear; do not include secrets.",
                            ),
                    ),
            )
            .when(self.error.is_some(), |body| {
                body.child(if self.confirm_discard {
                    Alert::warning(
                        "clipboard-editor-error",
                        self.error.clone().unwrap_or_default(),
                    )
                    .small()
                } else {
                    Alert::error(
                        "clipboard-editor-error",
                        self.error.clone().unwrap_or_default(),
                    )
                    .small()
                })
            })
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Ctrl/Cmd+S save · Esc cancel"),
                    )
                    .child(
                        Button::new("save-clipboard-edit")
                            .label("Save")
                            .primary()
                            .small()
                            .loading(self.saving)
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, w, cx| this.save(w, cx))),
                    )
                    .child(
                        Button::new("cancel-clipboard-edit")
                            .label("Cancel")
                            .small()
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, w, cx| this.cancel(w, cx))),
                    ),
            )
    }
}

impl Drop for ClipboardEditor {
    fn drop(&mut self) {
        if let Some(vault) = &self.vault {
            vault.lock();
        }
    }
}
