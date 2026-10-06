//! Keyboard-first retrieval with password-authorized use/edit of existing encrypted rows.
//! Protect and encrypted edits use atomic, vault-generation-bound storage transitions.
mod editor;

use crate::clipboard::{
    items::{ClipboardItem, ClipboardItemKind},
    storage::{ClipboardHistoryStore, RetentionFlag},
    vault_controller::VaultController,
};
use crate::gpui_component::{
    ActiveTheme, Disableable, Icon, Root, Selectable, Sizable, WindowExt,
    alert::Alert,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use crate::native::clipboard::{CaptureFilter, foreground, paste_foreground, write_clipboard_text};
use crate::native::components::IconName;
use crate::utils::time::now_unix_millis;
use gpui::{
    App, Bounds, Context, Entity, EntityInputHandler, Focusable, IntoElement, MouseButton,
    MouseDownEvent, Render, ScrollStrategy, Subscription, UniformListScrollHandle, Window,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowKind, WindowOptions, div,
    prelude::*, px, size, uniform_list,
};
use std::sync::{Arc, OnceLock, RwLock};

const WIDTH: f32 = 380.0;
const HEIGHT: f32 = 520.0;
const PAGE: isize = 6;
#[derive(Clone, Copy)]
enum AuthorizedAction {
    Protect,
    Unprotect,
    Use(bool),
    Edit,
}
struct PasswordPrompt {
    id: i64,
    action: AuthorizedAction,
    expected_plaintext: Option<zeroize::Zeroizing<String>>,
}
enum AuthorizedResult {
    Protected,
    Unprotected,
    Use(ClipboardItem, zeroize::Zeroizing<String>, bool),
    Edit(
        ClipboardItem,
        zeroize::Zeroizing<String>,
        Arc<VaultController>,
    ),
}

static GLOBAL_CAPTURE_FILTER: OnceLock<Arc<CaptureFilter>> = OnceLock::new();
pub fn set_global_capture_filter(filter: Arc<CaptureFilter>) {
    let _ = GLOBAL_CAPTURE_FILTER.set(filter);
}
// Keep bootstrap integration; never use session-unlocked state to bypass row authorization.
static GLOBAL_VAULT: OnceLock<Arc<VaultController>> = OnceLock::new();
pub fn set_global_vault_controller(vault: Arc<VaultController>) {
    let _ = GLOBAL_VAULT.set(vault);
}
pub(crate) static CLIPBOARD_POPUP: RwLock<Option<gpui::AnyWindowHandle>> = RwLock::new(None);

fn clear_popup() {
    if let Ok(mut slot) = CLIPBOARD_POPUP.write() {
        *slot = None;
    }
    super::set_clipboard_escape_target(false);
}
pub fn dispatch_escape(cx: &mut App) -> bool {
    let handle = CLIPBOARD_POPUP.read().ok().and_then(|slot| *slot);
    handle
        .and_then(|h| {
            h.update(cx, |_, w, cx| {
                w.dispatch_keystroke(gpui::Keystroke::parse("escape").expect("valid key"), cx)
            })
            .ok()
        })
        .unwrap_or(false)
}

pub fn open_from_main(
    cx: &mut App,
    events: std::sync::mpsc::Sender<super::StickerWindowEvent>,
) -> anyhow::Result<()> {
    let vault = GLOBAL_VAULT
        .get()
        .ok_or_else(|| anyhow::anyhow!("Clipboard backend unavailable"))?;
    let _ = vault;
    let store = GLOBAL_STORE
        .get()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Clipboard backend unavailable"))?;
    ClipboardChooserWindow::toggle(cx, events, store, None)
}
static GLOBAL_STORE: OnceLock<Arc<dyn ClipboardHistoryStore>> = OnceLock::new();
pub fn set_global_store(store: Arc<dyn ClipboardHistoryStore>) {
    let _ = GLOBAL_STORE.set(store);
}

pub struct ClipboardChooserWindow;
impl ClipboardChooserWindow {
    pub fn toggle(
        cx: &mut App,
        _events: std::sync::mpsc::Sender<super::StickerWindowEvent>,
        store: Arc<dyn ClipboardHistoryStore>,
        destination: Option<u64>,
    ) -> anyhow::Result<()> {
        let existing = CLIPBOARD_POPUP.read().ok().and_then(|slot| *slot);
        if let Some(handle) = existing {
            if dispatch_escape(cx) {
                return Ok(());
            }
            if handle.update(cx, |_, _, _| ()).is_ok() {
                return Ok(());
            }
            clear_popup();
        }
        let filter = GLOBAL_CAPTURE_FILTER
            .get()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Clipboard backend unavailable"))?;
        let bounds = Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx);
        let handle = cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_background: WindowBackgroundAppearance::Transparent,
                is_resizable: true,
                window_min_size: Some(size(px(380.0), px(360.0))),
                kind: WindowKind::Floating,
                titlebar: None,
                ..Default::default()
            },
            move |window, cx| {
                let view = cx
                    .new(|cx| ClipboardChooserWidget::new(window, cx, store, filter, destination));
                let surface = cx.new(|_| ClipboardChooserSurface { view });
                cx.new(|cx| Root::new(surface, window, cx))
            },
        )?;
        if let Ok(mut slot) = CLIPBOARD_POPUP.write() {
            *slot = Some(handle.into());
        }
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, w, _| w.activate_window());
        });
        Ok(())
    }
}

// Dialog builders read the chooser: render them outside its mutable Render borrow.
struct ClipboardChooserSurface {
    view: Entity<ClipboardChooserWidget>,
}
impl Render for ClipboardChooserSurface {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().relative().size_full().child(self.view.clone())
    }
}

fn validate_new_password(password: &str, confirmation: &str) -> Result<(), &'static str> {
    if password.is_empty() {
        return Err("Enter your master password.");
    }
    if password != confirmation {
        return Err("Passwords do not match. Enter the same password in both fields.");
    }
    Ok(())
}

fn protected(item: &ClipboardItem) -> bool {
    item.kind == ClipboardItemKind::Protected || item.retention.protected()
}
/// Never use a possibly stale plaintext preview of a protected row.
fn display_preview(item: &ClipboardItem) -> String {
    let text = if protected(item) {
        item.public_label.as_deref().unwrap_or("Protected item")
    } else {
        &item.content
    };
    text.chars()
        .take(120)
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}
fn redact_protected_previews(rows: &mut [ClipboardItem]) {
    for row in rows.iter_mut().filter(|r| protected(r)) {
        row.plain_preview = None;
        row.content.clear();
        row.kind = ClipboardItemKind::Protected;
    }
}

pub struct ClipboardChooserWidget {
    store: Arc<dyn ClipboardHistoryStore>,
    filter: Arc<CaptureFilter>,
    rows: Vec<ClipboardItem>,
    filtered: Vec<usize>,
    selected: usize,
    query: Entity<InputState>,
    scroll: UniformListScrollHandle,
    destination: Option<u64>,
    busy: bool,
    loading: bool,
    status: String,
    status_error: bool,
    status_warning: bool,
    password_error: Option<String>,
    password: Entity<InputState>,
    confirmation: Entity<InputState>,
    label: Entity<InputState>,
    creating_vault: bool,
    editor_open: bool,
    hovered_row: Option<i64>,
    prompt: Option<PasswordPrompt>,
    capture_enabled: bool,
    capture_status: String,
    query_version: u64,
    _keys: Subscription,
    _activation: Subscription,
}
impl ClipboardChooserWidget {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        store: Arc<dyn ClipboardHistoryStore>,
        filter: Arc<CaptureFilter>,
        destination: Option<u64>,
    ) -> Self {
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search clipboard history…"));
        cx.subscribe_in(&query, window, |this, query, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.filtered =
                    crate::clipboard::filter::filter_indices(&this.rows, &query.read(cx).value());
                this.selected = 0;
                this.scroll_to_selection();
                cx.notify();
            }
        })
        .detach();
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Master password")
        });
        cx.subscribe_in(&password, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::Change) && this.password_error.take().is_some() {
                cx.notify();
            }
        })
        .detach();
        let confirmation = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Confirm master password")
        });
        cx.subscribe_in(&confirmation, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::Change) && this.password_error.take().is_some() {
                cx.notify();
            }
        })
        .detach();
        let label = cx.new(|cx| InputState::new(window, cx).placeholder("Public label (required)"));
        cx.subscribe_in(&label, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::Change) && this.password_error.take().is_some() {
                cx.notify();
            }
        })
        .detach();
        let own_id = window.window_handle().window_id();
        let entity = cx.weak_entity();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle().window_id() != own_id || !window.is_window_active() {
                return;
            }
            let Some(entity) = entity.upgrade() else {
                return;
            };
            let key = event.keystroke.key.as_str();
            let mods = &event.keystroke.modifiers;
            let primary = if cfg!(target_os = "macos") {
                mods.platform || mods.control
            } else {
                mods.control
            };
            let handled = entity.update(cx, |this, cx| {
                if this.editor_open {
                    return false;
                }
                if this.prompt.is_some() {
                    if this
                        .password
                        .update(cx, |q, cx| q.marked_text_range(window, cx).is_some())
                    {
                        return false;
                    }
                    return match key {
                        "escape" if !this.busy => {
                            this.cancel_prompt(window, cx);
                            true
                        }
                        "enter" if !this.busy => {
                            this.submit_password(window, cx);
                            true
                        }
                        _ => false,
                    };
                }
                // IME candidate navigation/confirmation belongs to the text input.
                let composing = this
                    .query
                    .update(cx, |q, cx| q.marked_text_range(window, cx).is_some());
                if composing {
                    return false;
                }
                match key {
                    "escape" if !this.busy => {
                        window.remove_window();
                        clear_popup();
                        true
                    }
                    "enter" if !mods.alt && !mods.shift => {
                        this.use_selected(primary, window, cx);
                        true
                    }
                    "r" if primary && !mods.alt => {
                        this.reserve(window, cx);
                        true
                    }
                    "l" if primary && !mods.alt => {
                        this.protect_selected(window, cx);
                        true
                    }
                    "f2" => {
                        this.edit(window, cx);
                        true
                    }
                    "up" | "down" | "pageup" | "pagedown"
                        if !primary && !mods.alt && !mods.shift =>
                    {
                        let delta = match key {
                            "up" => -1,
                            "down" => 1,
                            "pageup" => -PAGE,
                            _ => PAGE,
                        };
                        this.selected = (this.selected as isize + delta)
                            .clamp(0, this.filtered.len().saturating_sub(1) as isize)
                            as usize;
                        this.scroll_to_selection();
                        cx.notify();
                        true
                    }
                    // Delete/Home/End retain their normal search-field editing behavior.
                    _ => false,
                }
            });
            if handled {
                cx.stop_propagation();
                window.prevent_default();
            }
        });
        let activation = cx.observe_window_activation(window, |_, window, _| {
            super::set_clipboard_escape_target(window.is_window_active());
        });
        window.on_window_should_close(cx, |_, _| {
            clear_popup();
            true
        });
        query.update(cx, |q, cx| q.focus(window, cx));
        let mut this = Self {
            store,
            filter,
            rows: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            query,
            scroll: UniformListScrollHandle::new(),
            destination,
            busy: false,
            loading: true,
            status: String::new(),
            status_error: false,
            status_warning: false,
            password_error: None,
            password,
            confirmation,
            label,
            creating_vault: false,
            editor_open: false,
            hovered_row: None,
            prompt: None,
            capture_enabled: false,
            capture_status: "Collector starting…".into(),
            query_version: 0,
            _keys: keys,
            _activation: activation,
        };
        this.refresh(window, cx);
        cx.spawn_in(window, async move |entity, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                if entity
                    .update_in(cx, |this, window, cx| {
                        this.capture_status = crate::native::clipboard::watcher::capture_status();
                        if !this.busy && this.prompt.is_none() && !this.editor_open {
                            this.refresh(window, cx);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        this
    }
    fn scroll_to_selection(&self) {
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Top);
    }
    fn selected_item(&self) -> Option<ClipboardItem> {
        self.rows.get(*self.filtered.get(self.selected)?).cloned()
    }
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let selected_id = self.selected_item().map(|r| r.id);
        self.query_version += 1;
        let version = self.query_version;
        cx.spawn_in(window, async move |entity, cx| {
            let result = store.snapshot().await;
            let settings = store.settings().await;
            let _ = entity.update_in(cx, |this, _, cx| {
                if version != this.query_version {
                    return;
                }
                if let Ok(settings) = settings {
                    this.capture_enabled = settings.capture_enabled;
                }
                this.loading = false;
                match result {
                    Ok(mut rows) => {
                        redact_protected_previews(&mut rows);
                        this.rows = rows;
                        this.filtered = crate::clipboard::filter::filter_indices(
                            &this.rows,
                            &this.query.read(cx).value(),
                        );
                        this.selected = selected_id
                            .and_then(|id| {
                                this.filtered.iter().position(|i| this.rows[*i].id == id)
                            })
                            .unwrap_or_else(|| {
                                this.selected.min(this.filtered.len().saturating_sub(1))
                            });
                        this.scroll_to_selection();
                    }
                    Err(_) => {
                        this.status_error = true;
                        this.status = "Could not load history. Try refreshing.".into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn use_selected(&mut self, copy_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(item) = self.selected_item() else {
            return;
        };
        if protected(&item) {
            self.request_password(item.id, AuthorizedAction::Use(copy_only), window, cx);
            return;
        }
        self.busy = true;
        self.status_error = false;
        self.status = if copy_only {
            "Copying…"
        } else {
            "Restoring destination…"
        }
        .into();
        cx.notify();
        let store = self.store.clone();
        let filter = self.filter.clone();
        let destination = self.destination;
        cx.spawn_in(window, async move |entity, cx| {
            // Re-read the chosen stable ID before any side effect; never use stale cached content.
            let result = async {
                let current = store.get(item.id).await?;
                anyhow::ensure!(
                    !protected(&current),
                    "This item is now protected. Nothing was copied."
                );
                if !copy_only {
                    entity.update_in(cx, |_, window, _| hide_chooser(window))??;
                }
                let dispatch = cx
                    .background_executor()
                    .spawn(async move {
                        if !copy_only {
                            crate::native::clipboard::paste::wait_for_modifiers()?;
                            foreground(destination)?;
                        }
                        write_clipboard_text(&filter, &current.content)?;
                        if !copy_only {
                            paste_foreground(destination)?;
                        }
                        Ok::<_, anyhow::Error>(())
                    })
                    .await;
                dispatch?;
                store.mark_used(item.id, now_unix_millis()).await?;
                Ok::<_, anyhow::Error>(())
            }
            .await;
            let _ = entity.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(()) => {
                        clear_popup();
                        window.remove_window();
                    }
                    Err(err) => {
                        show_chooser(window);
                        this.status_error = true;
                        this.status = format!("{err}");
                        window.activate_window();
                        this.query.update(cx, |q, cx| q.focus(window, cx));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
    fn reserve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(item) = self.selected_item() else {
            return;
        };
        self.status.clear();
        self.status_warning = false;
        self.busy = true;
        let store = self.store.clone();
        cx.spawn_in(window, async move |entity, cx| {
            let result = async {
                let current = store.get(item.id).await?;
                store
                    .set_retention_flag(
                        item.id,
                        RetentionFlag::Explicit,
                        !current.retention.explicit(),
                    )
                    .await?;
                Ok::<_, anyhow::Error>((
                    !current.retention.explicit(),
                    current.retention.edited() || protected(&current),
                ))
            }
            .await;
            let _ = entity.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.status_warning = false;
                this.status_error = result.is_err();
                this.status = match result {
                    Ok((true, _)) => "Reserved. Kept outside ordinary history limits.".into(),
                    Ok((false, true)) => {
                        "Reservation removed; edited/protected retention still keeps this item."
                            .into()
                    }
                    Ok((false, false)) => {
                        "Reservation removed; ordinary history limits now apply.".into()
                    }
                    Err(_) => "Reservation failed; no successful change is claimed.".into(),
                };
                this.refresh(window, cx);
                cx.notify();
            });
        })
        .detach();
    }
    fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(item) = self.selected_item() else {
            return;
        };
        if protected(&item) {
            self.request_password(item.id, AuthorizedAction::Edit, window, cx);
            return;
        }
        self.editor_open = true;
        let parent = cx.weak_entity();
        if editor::open(window, cx, self.store.clone(), item, parent, None).is_err() {
            self.status_error = true;
            self.editor_open = false;
            self.status = "Could not open the editor. Try again.".into();
            cx.notify();
        }
    }
    fn protect_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.prompt.is_some() {
            return;
        }
        let Some(item) = self.selected_item() else {
            return;
        };
        if protected(&item) {
            self.request_password(item.id, AuthorizedAction::Unprotect, window, cx);
            return;
        }
        self.request_password(item.id, AuthorizedAction::Protect, window, cx);
    }
    fn request_password(
        &mut self,
        id: i64,
        action: AuthorizedAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        if GLOBAL_VAULT.get().is_none() {
            self.status_error = true;
            self.status = "Vault unavailable. Try reopening history.".into();
            cx.notify();
            return;
        }
        let expected_plaintext = if matches!(action, AuthorizedAction::Protect) {
            self.selected_item()
                .filter(|item| item.id == id)
                .map(|item| zeroize::Zeroizing::new(item.content))
        } else {
            None
        };
        self.status.clear();
        self.status_warning = false;
        self.password_error = None;
        self.label.update(cx, |p, cx| p.set_value("", window, cx));
        self.prompt = Some(PasswordPrompt {
            id,
            action,
            expected_plaintext,
        });
        self.password
            .update(cx, |p, cx| p.set_value("", window, cx));
        self.busy = true;
        let vault = GLOBAL_VAULT.get().cloned().expect("vault checked above");
        cx.spawn_in(window, async move |entity, cx| {
            let result = cx.background_executor().spawn(async move {
                futures::executor::block_on(vault.envelope())
            }).await;
            let _ = entity.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(envelope) => {
                        this.creating_vault = envelope.is_none();
                        if this.creating_vault && !matches!(action, AuthorizedAction::Protect) {
                            this.clear_password_prompt(window, cx);
                            this.status_error = true;
                            this.status = "The original vault is missing. Restore it to unlock protected items; a new vault cannot decrypt them.".into();
                        } else {
                            this.open_password_dialog(window, cx);
                            if matches!(action, AuthorizedAction::Protect) {
                                this.label.update(cx, |p, cx| p.focus(window, cx));
                            } else {
                                this.password.update(cx, |p, cx| p.focus(window, cx));
                            }
                        }
                    }
                    Err(_) => {
                        this.clear_password_prompt(window, cx);
                        this.status_error = true;
                        this.status = "Could not read the clipboard vault. No changes were saved. Try reopening history.".into();
                    }
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }
    fn open_password_dialog(&self, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let this = entity.read(cx);
            let protecting = this.prompt.as_ref().is_some_and(|p| matches!(p.action, AuthorizedAction::Protect));
            let unprotecting = this.prompt.as_ref().is_some_and(|p| matches!(p.action, AuthorizedAction::Unprotect));
            let creating = this.creating_vault;
            let label = this.label.clone();
            let confirmation = this.confirmation.clone();
            let busy = this.busy;
            let password = this.password.clone();
            let error = this.password_error.clone();
            let submit = entity.clone();
            let cancel = entity.clone();
            let closed = entity.clone();
            dialog
                .title(if creating { "Set up clipboard vault" } else if protecting { "Protect clipboard item" } else if unprotecting { "Remove item protection" } else { "Unlock clipboard item" })
                .w(window.viewport_size().width.min(px(400.0)) - px(32.0))
                .text_sm()
                .overlay_closable(false)
                .keyboard(false)
                .close_button(!busy)
                .on_close(move |_, window, cx| {
                    closed.update(cx, |this, cx| {
                        if !this.busy && this.prompt.is_some() {
                            this.clear_password_prompt(window, cx);
                        }
                    });
                })
                .child(v_flex().gap_3()
                    .child(div().text_color(cx.theme().muted_foreground).child(
                        if creating { "Choose a master password to securely encrypt your clipboard items." } else if protecting { "Enter your master password to encrypt this item." }
                        else if unprotecting { "Enter your master password to remove encryption from this item." } else { "Enter your master password to use this protected item." }))
                    .when(creating, |body| body.child(Alert::warning("clipboard-vault-setup", "No clipboard vault exists yet. Keep your master password safe: it cannot be recovered or reset to unlock existing items.").small()))
                    .when(protecting, |body| body.child(Input::new(&label).disabled(busy).w_full())
                        .child(div().text_xs().text_color(cx.theme().muted_foreground).child("Visible in history without unlocking. Do not include secrets.")))
                    .child(Input::new(&password).disabled(busy).w_full())
                    .when(creating, |body| body.child(Input::new(&confirmation).disabled(busy).w_full()))
                    .when(error.is_some(), |body| body.child(Alert::error("clipboard-password-error", error.unwrap_or_default()).small()))
                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child(
                        if protecting { "This item is encrypted only after you confirm. Existing backups may still contain the original text." }
                        else if unprotecting { "The item will be stored as plain text and ordinary history limits will apply unless pinned or edited." } else { "Authorization is temporary. Protected copies clear after 30 seconds if unchanged." })))
                .footer(h_flex().items_center().gap_2()
                    .child(div().flex_1().text_xs().text_color(cx.theme().muted_foreground).child("Enter confirm · Esc cancel"))
                    .child(Button::new("cancel-authorization").label("Cancel").small().disabled(busy)
                        .on_click(move |_, window, cx| cancel.update(cx, |this, cx| this.cancel_prompt(window, cx))))
                    .child(Button::new("authorize-item").label(if creating { "Create vault & protect" } else if protecting { "Protect" } else if unprotecting { "Remove protection" } else { "Unlock" }).primary().small().loading(busy).disabled(busy)
                        .on_click(move |_, window, cx| submit.update(cx, |this, cx| this.submit_password(window, cx)))))
        });
    }

    fn clear_password_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt = None;
        self.creating_vault = false;
        self.confirmation
            .update(cx, |p, cx| p.set_value("", window, cx));
        self.password_error = None;
        self.password
            .update(cx, |p, cx| p.set_value("", window, cx));
        if let Some(vault) = GLOBAL_VAULT.get() {
            vault.lock();
        }
        self.query.update(cx, |q, cx| q.focus(window, cx));
        cx.notify();
    }
    fn cancel_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        self.clear_password_prompt(window, cx);
    }
    fn submit_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(prompt) = &self.prompt else {
            return;
        };
        let id = prompt.id;
        let action = prompt.action;
        let expected_plaintext = prompt.expected_plaintext.clone();
        let label = self.label.read(cx).value().trim().to_owned();
        if matches!(action, AuthorizedAction::Protect) && label.is_empty() {
            self.password_error = Some("Enter a public label for this item.".into());
            self.label.update(cx, |p, cx| p.focus(window, cx));
            cx.notify();
            return;
        }
        let creating = self.creating_vault;
        let password = zeroize::Zeroizing::new(self.password.read(cx).value().to_string());
        if creating {
            let confirmation =
                zeroize::Zeroizing::new(self.confirmation.read(cx).value().to_string());
            if let Err(message) = validate_new_password(&password, &confirmation) {
                self.password_error = Some(message.into());
                cx.notify();
                return;
            }
        }
        if password.is_empty() {
            self.password_error = Some("Enter your master password.".into());
            cx.notify();
            return;
        }
        self.password
            .update(cx, |p, cx| p.set_value("", window, cx));
        self.confirmation
            .update(cx, |p, cx| p.set_value("", window, cx));
        self.busy = true;
        self.password_error = None;
        cx.notify();
        let Some(vault) = GLOBAL_VAULT.get().cloned() else {
            self.busy = false;
            return;
        };
        let store = self.store.clone();
        let filter = self.filter.clone();
        let destination = self.destination;
        cx.spawn_in(window, async move |entity, cx| {
            let vault_for_work = vault.clone();
            let store_for_work = store.clone();
            let result = cx.background_executor().spawn(async move {
                futures::executor::block_on(async {
                    let failures = vault_for_work.failed_attempts().clamp(0, 5) as u64;
                    if failures > 0 { std::thread::sleep(std::time::Duration::from_millis(250 * failures)); }
                    if creating { vault_for_work.create(&password).await?; }
                    anyhow::ensure!(vault_for_work.unlock(&password).await?, "Wrong master password");
                    let current = store_for_work.get(id).await?;
                    if matches!(action, AuthorizedAction::Protect) {
                        let expected = expected_plaintext.as_ref().ok_or_else(|| anyhow::anyhow!("Original item unavailable; reload"))?;
                        anyhow::ensure!(current.kind == ClipboardItemKind::Plain && current.content == **expected, "Item changed; reload before protecting");
                        let sealed = vault_for_work.seal_for_storage(expected.as_bytes())?
                            .ok_or_else(|| anyhow::anyhow!("Authorization expired"))?;
                        store_for_work.compare_exchange_protection(id, expected, &sealed.ciphertext, &sealed.vault_envelope, Some(&label)).await?;
                        return Ok::<_, anyhow::Error>(AuthorizedResult::Protected);
                    }
                    anyhow::ensure!(current.kind == ClipboardItemKind::Protected, "Item changed; reload before use");
                    let bytes = vault_for_work.open(&current.content)?.ok_or_else(|| anyhow::anyhow!("Authorization expired"))?;
                    let text = zeroize::Zeroizing::new(std::str::from_utf8(&bytes)?.to_owned());
                    if matches!(action, AuthorizedAction::Unprotect) {
                        let binding = vault_for_work.seal_for_storage(text.as_bytes())?.ok_or_else(|| anyhow::anyhow!("Authorization expired"))?;
                        store_for_work.compare_exchange_unprotection(id, &current.content, &text, &binding.vault_envelope).await?;
                        return Ok(AuthorizedResult::Unprotected);
                    }
                    match action {
                        AuthorizedAction::Protect | AuthorizedAction::Unprotect => unreachable!("protection returns before decryption"),
                        AuthorizedAction::Use(copy_only) => {
                            Ok::<_, anyhow::Error>(AuthorizedResult::Use(current, text, copy_only))
                        }
                        AuthorizedAction::Edit => Ok(AuthorizedResult::Edit(current, text, vault_for_work.clone())),
                    }
                })
            }).await;
            let result = match result {
                Ok(AuthorizedResult::Use(item, text, copy_only)) => {
                    let hidden = if copy_only { Ok(()) } else {
                        entity.update_in(cx, |_, window, _| hide_chooser(window)).and_then(|r| r)
                    };
                    match hidden {
                        Err(err) => Err(err),
                        Ok(()) => {
                            let store = store.clone();
                            cx.background_executor().spawn(async move {
                                dispatch_content(&filter, &text, destination, copy_only, true)?;
                                futures::executor::block_on(store.mark_used(item.id, now_unix_millis()))?;
                                Ok::<_, anyhow::Error>(AuthorizedResult::Use(item, text, copy_only))
                            }).await
                        }
                    }
                }
                other => other,
            };
            // Creation can succeed before protection fails. Check persistence before
            // deciding whether retry should create or unlock; never replace a vault.
            let still_unconfigured = if creating {
                let vault = vault.clone();
                cx.background_executor().spawn(async move {
                    matches!(futures::executor::block_on(vault.envelope()), Ok(None))
                }).await
            } else { false };
            // One-shot use; only the protected editor retains a scoped key.
            let editor_result = matches!(result, Ok(AuthorizedResult::Edit(..)));
            if !editor_result { vault.lock(); }
            let update = entity.update_in(cx, |this, window, cx| {
                this.busy = false;
                // Creation may succeed even when subsequent protection fails. Never replace it on retry.
                if creating { this.creating_vault = still_unconfigured; }
                match result {
                    Ok(AuthorizedResult::Unprotected) => {
                        this.cancel_prompt(window, cx);
                        this.status_error = false;
                        this.status_warning = true;
                        this.status = "Protection removed. This item is now stored as plain text.".into();
                        this.refresh(window, cx);
                    }
                    Ok(AuthorizedResult::Protected) => {
                        this.cancel_prompt(window, cx);
                        this.status_error = false; this.status = "Item protected. Collection is paused to prevent recapturing it.".into();
                        // Erase the stale chooser snapshot before the asynchronous refresh.
                        for row in this.rows.iter_mut().filter(|row| row.id == id) {
                            row.content.clear(); row.plain_preview = None;
                            row.kind = ClipboardItemKind::Protected; row.retention.protected_ = true;
                        }
                        this.refresh(window, cx);
                    }
                    Ok(AuthorizedResult::Use(..)) => { this.cancel_prompt(window, cx); clear_popup(); window.remove_window(); }
                    Ok(AuthorizedResult::Edit(item, text, vault)) => {
                        window.close_dialog(cx);
                        this.prompt = None; this.password_error = None;
                        this.password.update(cx, |p, cx| p.set_value("", window, cx));
                        this.editor_open = true;
                        let parent = cx.weak_entity();
                        if editor::open(window, cx, this.store.clone(), item, parent, Some((text, vault.clone()))).is_err() { this.editor_open = false; vault.lock(); this.status_error = true; this.status = "Could not open the encrypted editor.".into(); }
                    }
                    Err(err) => {
                        show_chooser(window);
                        this.password_error = Some(if err.to_string().contains("No master password") {
                            "Set up your master password first: rusticker cp vault create."
                        } else if err.to_string() == "Wrong master password" {
                            "Incorrect master password. Try again."
                        } else if matches!(action, AuthorizedAction::Protect) {
                            "Could not protect this item. If setup succeeded, use your new master password. Refresh history if the item changed."
                        } else {
                            "Could not unlock or use this item. Check your password, or cancel and try Copy only."
                        }.into());
                        window.activate_window(); this.password.update(cx, |p, cx| p.focus(window, cx));
                    }
                }
                cx.notify();
            });
            if update.is_err() && editor_result { vault.lock(); }
        }).detach();
    }
    fn toggle_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.status.clear();
        self.status_warning = false;
        self.busy = true;
        let store = self.store.clone();
        cx.spawn_in(window, async move |entity, cx| {
            let result = async {
                let mut settings = store.settings().await?;
                settings.capture_enabled = !settings.capture_enabled;
                store.update_settings(&settings).await?;
                Ok::<_, anyhow::Error>(settings.capture_enabled)
            }
            .await;
            let _ = entity.update_in(cx, |this, _, cx| {
                this.busy = false;
                this.status_warning = false;
                this.status_error = result.is_err();
                match result {
                    Ok(enabled) => {
                        this.capture_enabled = enabled;
                        this.status_warning = enabled;
                        this.status = if enabled {
                            "Collection enabled. Copied text, including secrets, will be saved."
                        } else {
                            "Collection paused. Your existing history is still available."
                        }
                        .into();
                    }
                    Err(_) => {
                        this.status =
                            "Could not update collection. Your previous setting is unchanged."
                                .into()
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn remove_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.prompt.is_some() || self.editor_open {
            return;
        }
        let Some(item) = self.selected_item() else {
            return;
        };
        self.busy = true;
        let store = self.store.clone();
        cx.spawn_in(window, async move |entity, cx| {
            let result = store.delete_row(item.id).await;
            let _ = entity.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.status_error = result.is_err();
                this.status_warning = false;
                this.status = if result.is_ok() {
                    "Item removed."
                } else {
                    "Could not remove this item. Try again."
                }
                .into();
                this.refresh(window, cx);
            });
        })
        .detach();
    }
    fn row(&self, index: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let item = &self.rows[self.filtered[index]];
        let pinned = item.retention.explicit();
        let locked = protected(item);
        let disabled = self.busy || self.prompt.is_some() || self.editor_open;
        let hovered = self.hovered_row == Some(item.id);
        let id = item.id;
        let actions = h_flex()
            .gap_0()
            .flex_shrink_0()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(!locked && hovered, |actions| {
                actions.child(
                    div().when(!hovered, |el| el.hidden()).child(
                        Button::new(("row-pin", item.id as usize))
                            .icon(IconName::Pin)
                            .ghost()
                            .small()
                            .selected(pinned)
                            .disabled(disabled)
                            .tooltip(if pinned {
                                "Unpin · Ctrl/Cmd+R"
                            } else {
                                "Pin · Ctrl/Cmd+R"
                            })
                            .on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.selected = index;
                                this.reserve(w, cx);
                            })),
                    ),
                )
            })
            .child(
                div().when(!hovered, |el| el.hidden()).child(
                    Button::new(("row-copy", item.id as usize))
                        .icon(IconName::Copy)
                        .ghost()
                        .small()
                        .disabled(disabled)
                        .tooltip("Copy only · Ctrl/Cmd+Enter")
                        .on_click(cx.listener(move |this, _, w, cx| {
                            cx.stop_propagation();
                            this.selected = index;
                            this.use_selected(true, w, cx);
                        })),
                ),
            )
            .child(
                div()
                    .when(!locked, |el| el.when(!hovered, |el| el.hidden()))
                    .child(
                        Button::new(("row-lock", item.id as usize))
                            .icon(if locked {
                                IconName::LockClosed
                            } else {
                                IconName::LockOpen
                            })
                            .ghost()
                            .small()
                            .selected(locked)
                            .disabled(disabled)
                            .tooltip(if locked {
                                "Remove protection · master password required · Ctrl/Cmd+L"
                            } else {
                                "Protect with a public label · Ctrl/Cmd+L"
                            })
                            .on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.selected = index;
                                this.protect_selected(w, cx);
                            })),
                    ),
            )
            .child(
                div().when(!hovered, |el| el.hidden()).child(
                    Button::new(("row-remove", item.id as usize))
                        .icon(IconName::Delete)
                        .ghost()
                        .small()
                        .disabled(disabled)
                        .tooltip("Delete this item from clipboard history")
                        .on_click(cx.listener(move |this, _, w, cx| {
                            cx.stop_propagation();
                            this.selected = index;
                            this.remove_selected(w, cx);
                        })),
                ),
            );
        h_flex()
            .id(("clipboard-row", item.id as usize))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered_row = Some(id);
                } else if this.hovered_row == Some(id) {
                    this.hovered_row = None;
                }
                cx.notify();
            }))
            .w_full()
            .items_center()
            .h(px(44.0))
            .px_2()
            .gap_1()
            .rounded_md()
            .border_1()
            .border_color(if index == self.selected {
                cx.theme().list_active_border
            } else {
                cx.theme().transparent
            })
            .bg(if index == self.selected {
                cx.theme().list_active
            } else {
                cx.theme().transparent
            })
            .hover(|row| row.bg(cx.theme().list_hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if this.busy || this.prompt.is_some() || this.editor_open {
                        return;
                    }
                    this.selected = index;
                    this.query.update(cx, |q, cx| q.focus(window, cx));
                    if event.click_count == 2 {
                        this.edit(window, cx);
                    }
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .truncate()
                    .child(display_preview(item)),
            )
            .when(hovered || locked, |row| row.child(actions))
            .into_any_element()
    }
}
impl Focusable for ClipboardChooserWidget {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.query.read(cx).focus_handle(cx)
    }
}
impl Render for ClipboardChooserWidget {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let count = self.filtered.len();
        let disabled = self.busy || self.prompt.is_some() || self.editor_open;
        let has_protected = self.rows.iter().any(protected);
        let collector_label = if !self.capture_enabled {
            "Collection paused"
        } else if has_protected {
            "Collection paused · protected history"
        } else {
            "Collecting"
        };
        v_flex()
            .relative()
            .size_full()
            .rounded_lg()
            .overflow_hidden()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .window_control_area(WindowControlArea::Drag)
                    .child(div().flex_1().text_sm().child("Clipboard history"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{count} items")),
                    )
                    .child(
                        Button::new("refresh")
                            .icon(IconName::Refresh)
                            .ghost()
                            .small()
                            .tooltip("Refresh history")
                            .occlude()
                            .disabled(disabled)
                            .on_click(cx.listener(|this, _, w, cx| this.refresh(w, cx))),
                    )
                    .child(
                        Button::new("close-clipboard")
                            .icon(IconName::Close)
                            .ghost()
                            .small()
                            .tooltip("Close · Esc")
                            .occlude()
                            .disabled(disabled)
                            .on_click(cx.listener(|_, _, window, _| {
                                clear_popup();
                                window.remove_window();
                            })),
                    ),
            )
            .child(
                div().px_3().pb_2().flex_shrink_0().child(
                    Input::new(&self.query)
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search))
                        .disabled(disabled)
                        .w_full(),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .px_2()
                    .overflow_hidden()
                    .when(count > 0, |el| {
                        el.child(
                            uniform_list("clipboard-history", count, move |range, _, cx| {
                                view.update(cx, |this, cx| {
                                    range.map(|index| this.row(index, cx)).collect::<Vec<_>>()
                                })
                            })
                            .size_full()
                            .track_scroll(&self.scroll),
                        )
                    })
                    .when(count == 0, |el| {
                        el.child(
                            v_flex()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .gap_2()
                                .px_4()
                                .child(Icon::new(IconName::DocumentText).with_size(px(24.0)))
                                .child(div().text_sm().child(if self.loading {
                                    "Loading history…"
                                } else if self.rows.is_empty() {
                                    "Your copied text will appear here"
                                } else {
                                    "No matching items"
                                }))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if self.rows.is_empty() {
                                            "Enable collection below to get started."
                                        } else {
                                            "Try a different search or clear the filter."
                                        }),
                                ),
                        )
                    }),
            )
            .child(
                v_flex()
                    .flex_shrink_0()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("↑↓ · Enter paste · F2 edit · Esc"),
                            )
                            .child(
                                Button::new("toggle-collection")
                                    .label(if self.capture_enabled {
                                        "Pause"
                                    } else {
                                        "Collect"
                                    })
                                    .icon(if self.capture_enabled {
                                        IconName::Pause
                                    } else {
                                        IconName::Play
                                    })
                                    .ghost()
                                    .small()
                                    .tooltip(if self.capture_enabled {
                                        collector_label
                                    } else {
                                        "Enable collection. Copied secrets can enter history."
                                    })
                                    .disabled(disabled)
                                    .on_click(
                                        cx.listener(|this, _, w, cx| this.toggle_capture(w, cx)),
                                    ),
                            ),
                    )
                    .when(!self.status.is_empty() && self.prompt.is_none(), |footer| {
                        footer.child(if self.status_error {
                            Alert::error("clipboard-status", self.status.clone()).small()
                        } else if self.status_warning {
                            Alert::warning("clipboard-status", self.status.clone()).small()
                        } else {
                            Alert::info("clipboard-status", self.status.clone()).small()
                        })
                    }),
            )
    }
}

fn dispatch_content(
    filter: &Arc<CaptureFilter>,
    content: &str,
    destination: Option<u64>,
    copy_only: bool,
    secret: bool,
) -> anyhow::Result<()> {
    if secret {
        anyhow::ensure!(
            cfg!(target_os = "windows"),
            "Ownership-aware protected clipboard expiry is only available on Windows"
        );
    }
    if !copy_only {
        crate::native::clipboard::paste::wait_for_modifiers()?;
        foreground(destination)?;
    }
    let generation =
        crate::native::clipboard::watcher::write_clipboard_text_owned(filter, content)?;
    if secret {
        let filter = filter.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(30));
            let _ =
                crate::native::clipboard::watcher::clear_protected_if_owned(&filter, generation);
        });
    }
    if !copy_only {
        paste_foreground(destination)?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn hide_chooser(window: &Window) -> anyhow::Result<()> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let RawWindowHandle::Win32(handle) = HasWindowHandle::window_handle(window)?.as_raw() else {
        anyhow::bail!("No native chooser handle");
    };
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
            handle.hwnd.get() as _,
            windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
        );
    }
    super::set_clipboard_escape_target(false);
    Ok(())
}
#[cfg(not(target_os = "windows"))]
fn hide_chooser(_: &Window) -> anyhow::Result<()> {
    anyhow::bail!("Verified paste unavailable; use Copy only")
}
#[cfg(target_os = "windows")]
fn show_chooser(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(handle) = HasWindowHandle::window_handle(window)
        && let RawWindowHandle::Win32(handle) = handle.as_raw()
    {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                handle.hwnd.get() as _,
                windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW,
            );
        }
    }
}
#[cfg(not(target_os = "windows"))]
fn show_chooser(_: &Window) {}

#[cfg(test)]
mod tests {
    use super::*;
    fn item() -> ClipboardItem {
        ClipboardItem {
            id: 1,
            kind: ClipboardItemKind::Plain,
            content: "secret\n\t\u{1b}".into(),
            plain_preview: Some("secret".into()),
            public_label: None,
            byte_size: 9,
            created_at: 0,
            last_copied_at: 0,
            last_used_at: None,
            last_edited_at: None,
            retention: Default::default(),
        }
    }
    #[test]
    fn clipboard_action_icons_are_embedded() {
        use crate::gpui_component::IconNamed;
        for icon in [IconName::Copy, IconName::Delete] {
            assert!(crate::native::components::Assets::get(icon.path().as_ref()).is_some());
        }
    }
    #[test]
    fn vault_setup_requires_matching_passwords() {
        assert!(validate_new_password("", "").is_err());
        assert!(validate_new_password("a", "a").is_ok());
        assert!(validate_new_password(" ", " ").is_ok());
        assert!(validate_new_password("long-password", "different-password").is_err());
        assert!(validate_new_password("long-password", "long-password").is_ok());
    }
    #[test]
    fn protected_preview_never_falls_back_to_content() {
        let mut row = item();
        row.kind = ClipboardItemKind::Protected;
        assert_eq!(display_preview(&row), "Protected item");
        redact_protected_previews(std::slice::from_mut(&mut row));
        assert!(row.content.is_empty());
        assert!(row.plain_preview.is_none());
    }
    #[test]
    fn legacy_protection_flag_is_fail_closed() {
        let mut row = item();
        row.retention.protected_ = true;
        assert!(protected(&row));
        assert_eq!(display_preview(&row), "Protected item");
    }
    #[test]
    fn previews_escape_controls_without_changing_payload() {
        let row = item();
        assert!(!display_preview(&row).chars().any(char::is_control));
        assert!(row.content.contains('\n'));
    }
}
