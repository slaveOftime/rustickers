//! One password/settings overlay shared by TXTs and ordinary text stickers.
use crate::gpui_component::{
    ActiveTheme, Disableable, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use crate::text::TextRepository;
use gpui::{Context, Entity, Window, div, prelude::*, px};
use std::sync::{Arc, Mutex, OnceLock};
use zeroize::Zeroizing;

type PasswordCallback = Box<dyn FnOnce(Zeroizing<String>, &mut Window, &mut gpui::App)>;
static DIRTY_EDITORS: OnceLock<Mutex<std::collections::HashSet<i64>>> = OnceLock::new();
static ROTATING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn is_rotating() -> bool {
    ROTATING.load(std::sync::atomic::Ordering::Acquire)
}
pub(crate) fn set_dirty(id: i64, dirty: bool) {
    let mut editors = DIRTY_EDITORS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if dirty {
        editors.insert(id);
    } else {
        editors.remove(&id);
    }
}
pub(crate) fn has_dirty_editors() -> bool {
    !DIRTY_EDITORS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_empty()
}
struct PasswordForm {
    password: Entity<InputState>,
    new_password: Entity<InputState>,
    confirmation: Entity<InputState>,
    callback: Option<PasswordCallback>,
    repository: Option<Arc<TextRepository>>,
    configured: bool,
    busy: bool,
    error: Option<String>,
}
impl PasswordForm {
    fn clear(&self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [&self.password, &self.new_password, &self.confirmation] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let password = Zeroizing::new(self.password.read(cx).value().to_string());
        if let Some(repository) = self.repository.clone() {
            let new = Zeroizing::new(self.new_password.read(cx).value().to_string());
            let confirmation = Zeroizing::new(self.confirmation.read(cx).value().to_string());
            if new.is_empty() || new != confirmation {
                self.clear(window, cx);
                self.error =
                    Some("Enter a nonempty new password and matching confirmation.".into());
                cx.notify();
                return;
            }
            if has_dirty_editors() {
                self.error = Some(
                    "Save or discard unsaved encrypted text edits before changing the password."
                        .into(),
                );
                cx.notify();
                return;
            }
            if ROTATING.swap(true, std::sync::atomic::Ordering::AcqRel) {
                self.error = Some("Another password change is in progress.".into());
                cx.notify();
                return;
            }
            cx.refresh_windows();
            let configured = self.configured;
            self.busy = true;
            self.error = None;
            cx.notify();
            let entity = cx.entity();
            window.spawn(cx, async move |cx| {
                let result = cx.background_executor().spawn(async move {
                    futures::executor::block_on(async {
                        if configured { repository.change_master_password(&password, &new).await }
                        else { repository.set_master_password(&new).await }
                    })
                }).await;
                ROTATING.store(false,std::sync::atomic::Ordering::Release);
                let _ = entity.update_in(cx, |this, window, cx| {
                    cx.refresh_windows();
                    this.busy = false;
                    match result {
                        Ok(()) => { this.clear(window, cx); window.close_dialog(cx); cx.defer(super::super::sticker::StickerWindow::invalidate_protected_views); }
                        Err(_) => { this.clear(window,cx); this.error = Some("Could not change the master password. Check the current password and locked texts; nothing was changed.".into()); },
                    }
                    cx.notify();
                });
            }).detach();
        } else if let Some(callback) = self.callback.take() {
            self.clear(window, cx);
            window.close_dialog(cx);
            callback(password, window, cx);
        }
    }
}
fn open_form(
    window: &mut Window,
    cx: &mut gpui::App,
    title: &'static str,
    repository: Option<Arc<TextRepository>>,
    configured: bool,
    callback: Option<PasswordCallback>,
) {
    if window.has_active_dialog(cx) {
        return;
    }
    let form = cx.new(|cx| PasswordForm {
        password: cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Current master password")
        }),
        new_password: cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("New master password")
        }),
        confirmation: cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Confirm new password")
        }),
        callback,
        repository,
        configured,
        busy: false,
        error: None,
    });
    let focus = if form.read(cx).repository.is_some() && !configured {
        form.read(cx).new_password.clone()
    } else {
        form.read(cx).password.clone()
    };
    window.open_dialog(cx, move |dialog, _, cx| {
        let state = form.read(cx); let busy = state.busy; let settings = state.repository.is_some();
        let submit = form.clone(); let cancel = form.clone(); let closed = form.clone();
        dialog.max_w(px(300.)).title(title).overlay_closable(false).keyboard(false).close_button(!busy)
            .on_close(move |_, window, cx| closed.update(cx, |this, cx| { this.clear(window, cx); this.callback = None; }))
            .child(v_flex().py_2().gap_3().on_key_down({ let form = form.clone(); move |event: &gpui::KeyDownEvent,window,cx| {
                if event.keystroke.key == "enter" { form.update(cx,|this,cx|this.submit(window,cx)); cx.stop_propagation(); }
                else if event.keystroke.key == "escape" { if !form.read(cx).busy { form.update(cx,|this,cx|{this.clear(window,cx);this.callback=None;});window.close_dialog(cx); } cx.stop_propagation(); }
            }})
                .when(!settings || configured, |body| body.child(Input::new(&state.password).disabled(busy)))
                .when(settings, |body| body.child(div().child("New password")).child(Input::new(&state.new_password).disabled(busy))
                    .child(div().child("Confirm password")).child(Input::new(&state.confirmation).disabled(busy))
                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child(if configured {
                        "Every locked text will be re-encrypted. Keep your password safe; there is no recovery. Old backups are not erased."
                    } else { "Keep this password safe. Forgotten passwords cannot recover locked texts." })))
                .when(busy, |body| body.child("Re-encrypting locked texts. Do not close the app."))
                .when_some(state.error.clone(), |body, error| body.child(div().text_color(cx.theme().danger).child(error))))
            .footer(h_flex().justify_end().gap_2().child(Button::new("password-cancel").label("Cancel").disabled(busy)
                .on_click(move |_, window, cx| { cancel.update(cx, |this, cx| { this.clear(window,cx); this.callback=None; }); window.close_dialog(cx); }))
                .child(Button::new("password-submit").label(if settings { if configured { "Change password" } else { "Set password" } } else { "Unlock" })
                    .primary().loading(busy).disabled(busy).on_click(move |_, window, cx| submit.update(cx, |this,cx| this.submit(window,cx)))))
    });
    focus.update(cx, |input, cx| input.focus(window, cx));
}
pub(crate) fn unlock(window: &mut Window, cx: &mut gpui::App, callback: PasswordCallback) {
    open_form(window, cx, "Unlock text", None, false, Some(callback));
}
pub(crate) fn settings(window: &mut Window, cx: &mut gpui::App, repository: Arc<TextRepository>) {
    if window.has_active_dialog(cx) || is_rotating() {
        return;
    }
    let executor = cx.background_executor().clone();
    window
        .spawn(cx, async move |cx| {
            let probe = repository.clone();
            let result = executor
                .spawn(async move { futures::executor::block_on(probe.master_key()) })
                .await;
            let _ = cx.update(|window, cx| match result {
                Ok(key) => open_form(
                    window,
                    cx,
                    if key.is_some() {
                        "Change master password"
                    } else {
                        "Set master password"
                    },
                    Some(repository),
                    key.is_some(),
                    None,
                ),
                Err(_) => {
                    window.open_alert_dialog(cx, |dialog, _, _| {
                        dialog
                            .title("Master password unavailable")
                            .child("The stored key configuration could not be read.")
                    });
                }
            });
        })
        .detach();
}
