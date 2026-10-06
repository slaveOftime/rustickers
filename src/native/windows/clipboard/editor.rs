//! In-history modal editor with atomic saves and scoped protected authorization.
use super::{ClipboardChooserWidget, protected};
use crate::clipboard::{items::ClipboardItem, storage::ClipboardHistoryStore};

use gpui::{
    App, Context, Entity, Focusable, IntoElement, Render, Subscription, WeakEntity, Window, black,
    div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Sizable, WindowExt,
    alert::Alert,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Textarea, TextareaState},
    v_flex,
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
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !protected(&initial) || authorized.is_some(),
        "Encrypted editing requires authorization"
    );
    let view = cx.new(|cx| ClipboardEditor::new(window, cx, store, initial, parent, authorized));
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
            .bg(black().opacity(0.95))
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
    ) -> Self {
        let (original_plain, vault) = match authorized {
            Some((text, vault)) => (text, Some(vault)),
            None => (zeroize::Zeroizing::new(initial.content.clone()), None),
        };
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
        if self.content.read(cx).value().as_str() != self.original_plain.as_str()
            && !self.confirm_discard
        {
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
        if *text == *self.original_plain {
            self.close(window, cx);
            return;
        }
        self.saving = true;
        self.error = None;
        cx.notify();
        let store = self.store.clone();
        let initial = self.initial.clone();
        let parent = self.parent.clone();
        let vault = self.vault.clone();
        cx.spawn_in(window, async move |entity, cx| {
            let result = async {
                if let Some(vault) = vault {
                    let sealed = vault.seal_for_storage(text.as_bytes())?.ok_or_else(|| anyhow::anyhow!("Authorization expired; reopen to unlock"))?;
                    store.compare_exchange_protected_content_bound(initial.id, &initial.content, &sealed.ciphertext, text.len() as i64, crate::utils::time::now_unix_millis(), &sealed.vault_envelope).await
                } else {
                    // Compare-and-swap is atomic with edited retention and rejects protection/deletion races.
                    store.compare_exchange_content(initial.id, &initial.content, &text, text.len() as i64, crate::utils::time::now_unix_millis()).await
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
            .bg(black().opacity(0.90))
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .h(px(200.0))
                    .min_h_0()
                    .child(Textarea::new(&self.content).size_full()),
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
