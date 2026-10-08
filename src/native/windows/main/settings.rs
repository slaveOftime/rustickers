//! Manager settings reuse the text service and existing authorization dialogs.
use super::*;
use crate::gpui_component::{scroll::ScrollableElement, switch::Switch};
use crate::native::windows::text;

impl MainWindow {
    pub(super) fn save_shortcuts(&mut self, cx: &mut Context<Self>) {
        if self.shortcuts_busy || !self.shortcuts_dirty {
            return;
        }
        let values = std::array::from_fn(|ix| self.shortcuts[ix].read(cx).value().to_string());
        self.shortcuts_busy = true;
        self.shortcuts_error = None;
        cx.notify();
        cx.spawn(async move |entity, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::native::hotkey::save_bindings(values) })
                .await;
            let _ = entity.update(cx, |this, cx| {
                this.shortcuts_busy = false;
                match result {
                    Ok(()) => this.shortcuts_dirty = false,
                    Err(error) => this.shortcuts_error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn reset_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shortcuts_busy {
            return;
        }
        let values = crate::native::hotkey::default_bindings();
        for (input, value) in self.shortcuts.iter().zip(values) {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        self.shortcuts_dirty = true;
        self.shortcuts_error = None;
        cx.notify();
    }
    fn shortcuts_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        v_flex().gap_3().border_t_1().border_color(cx.theme().border).pt_5()
            .child(heading("Global shortcuts"))
            .child(help("Use Ctrl, Alt, Shift, a letter, digit, F1–F12, or Space. On macOS, Ctrl also matches Cmd."))
            .children(["Show manager", "Open TXTs", "Quick file preview", "Selection command"].into_iter().enumerate().map(|(ix, label)| {
                v_flex().gap_1()
                    .child(div().text_sm().child(label))
                    .child(Input::new(&self.shortcuts[ix]).small().disabled(self.shortcuts_busy))
            }))
            .child(help("Only quick file preview may use modifiers alone; it fires on release. Avoid shortcuts already used by other apps."))
            .when_some(self.shortcuts_error.as_ref(), |v, error| v.child(div().text_xs().text_color(cx.theme().danger).child(error.clone())))
            .child(h_flex().gap_2()
                .child(Button::new("save-global-shortcuts").small().primary()
                    .label(if self.shortcuts_busy { "Saving…" } else if self.shortcuts_dirty { "Save shortcuts" } else { "Shortcuts saved" })
                    .disabled(self.shortcuts_busy || !self.shortcuts_dirty)
                    .on_click(cx.listener(|this, _, _, cx| this.save_shortcuts(cx))))
                .child(Button::new("reset-global-shortcuts").small().ghost().label("Use defaults")
                    .disabled(self.shortcuts_busy)
                    .on_click(cx.listener(|this, _, window, cx| this.reset_shortcuts(window, cx)))))
            .into_any_element()
    }
    pub(super) fn load_settings(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = text::text_repository() else {
            return;
        };
        self.settings_revision = self.settings_revision.wrapping_add(1);
        let revision = self.settings_revision;
        cx.spawn(async move |entity, cx| {
            let result =
                async { anyhow::Ok((repo.settings().await?, repo.master_key().await?.is_some())) }
                    .await;
            let _ = entity.update(cx, |this, cx| {
                if revision != this.settings_revision {
                    return;
                }
                match result {
                    Ok((settings, configured)) => {
                        this.clipboard_settings = Some(settings);
                        this.password_configured = Some(configured);
                    }
                    Err(_) => this.error = Some("Could not load settings. Try again.".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn set_capture(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.settings_busy {
            return;
        }
        let Some(repo) = text::text_repository() else {
            return;
        };
        self.settings_busy = true;
        self.settings_revision = self.settings_revision.wrapping_add(1);
        cx.notify();
        cx.spawn(async move |entity, cx| {
            let result = async {
                let mut settings = repo.settings().await?;
                settings.capture_enabled = enabled;
                repo.update_settings(&settings).await?;
                anyhow::Ok(settings)
            }
            .await;
            let _ = entity.update(cx, |this, cx| {
                this.settings_busy = false;
                this.settings_revision = this.settings_revision.wrapping_add(1);
                match result {
                    Ok(settings) => {
                        this.clipboard_settings = Some(settings);
                        this.error = None;
                    }
                    Err(_) => {
                        this.error =
                            Some("Could not update clipboard collection. Try again.".into())
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn settings_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let settings = self.clipboard_settings.as_ref();
        div().flex_1().min_h_0().overflow_y_scrollbar().child(v_flex().p_5().gap_6()
            .child(v_flex().gap_3()
                .child(heading("Text protection"))
                .child(h_flex().gap_4().items_start()
                    .child(v_flex().flex_1().min_w_0().gap_1()
                        .child(div().text_sm().font_medium().child(match self.password_configured { Some(true) => "Master password configured", Some(false) => "No master password", None => "Checking protection…" }))
                        .child(help("Encryption needs no password. Each unlock is authorized separately.")))
                    .child(Button::new("manager-password").small().label(if self.password_configured == Some(true) { "Change password…" } else { "Set password…" })
                        .disabled(self.password_configured.is_none() || text::password::is_rotating())
                        .on_click(|_, window, cx| { if let Some(repo) = text::text_repository() { text::password::settings(window, cx, repo); } })))
                .child(help("Titles remain public and searchable. There is no forgotten-password recovery.")))
            .child(v_flex().gap_3().border_t_1().border_color(cx.theme().border).pt_5()
                .child(heading("Clipboard history"))
                .child(h_flex().gap_4()
                    .child(v_flex().flex_1().min_w_0().gap_1().child(div().text_sm().font_medium().child("Collect copied text")).child(help("Disposable captures appear in TXTs. Saved stickers are kept separately.")))
                    .child(Switch::new("manager-capture").accessibility_label("Collect copied text").checked(settings.is_some_and(|s| s.capture_enabled)).disabled(self.settings_busy || settings.is_none())
                        .on_change(cx.listener(|this, enabled, _, cx| this.set_capture(*enabled, cx)))))
                .when_some(settings, |v, s| v.child(help(format!("History budget: {} captures / {} MiB. Collection does not erase OS clipboard history.", s.normal_count_limit, s.normal_byte_limit / (1024 * 1024))))))
            .child(self.shortcuts_view(cx))
            .child(v_flex().gap_2().border_t_1().border_color(cx.theme().border).pt_5()
                .child(heading("Local data"))
                .child(help("Stickers and captures share a local SQLite database. Protect database backups too."))
                .child(help(format!("Rustickers {}", env!("CARGO_PKG_VERSION"))))))
            .into_any_element()
    }
}
fn heading(label: &'static str) -> gpui::Div {
    div().text_sm().font_semibold().child(label)
}
fn help(label: impl Into<gpui::SharedString>) -> gpui::Div {
    div().text_xs().child(label.into())
}
