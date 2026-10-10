//! TXTs: one keyboard-first picker over saved text stickers and disposable captures.
use crate::gpui_component::{
    ActiveTheme, Disableable, Icon, Root, Selectable, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use crate::{
    native::{
        clipboard::{CaptureFilter, foreground, paste_foreground},
        components::IconName,
        windows::{StickerWindowEvent, sticker::StickerWindow},
    },
    storage::ArcStickerStore,
    text::{TextEntry, TextEntryId, TextRepository, TextSources, filter_indices},
};
use gpui::{
    App, Bounds, Context, Entity, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, Render,
    ScrollStrategy, Subscription, UniformListScrollHandle, Window, WindowBackgroundAppearance,
    WindowBounds, WindowControlArea, WindowKind, WindowOptions, div, prelude::*, px, size,
    uniform_list,
};
use std::sync::{Arc, OnceLock, RwLock};
use zeroize::Zeroizing;

static GLOBAL_REPOSITORY: OnceLock<Arc<TextRepository>> = OnceLock::new();
static GLOBAL_STORE: OnceLock<ArcStickerStore> = OnceLock::new();
static OWNED_SECRET: std::sync::Mutex<Option<(Arc<CaptureFilter>, Option<u32>)>> =
    std::sync::Mutex::new(None);
pub(crate) fn clear_owned_secret() {
    if let Some((filter, generation)) = OWNED_SECRET
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        let _ = crate::native::clipboard::watcher::clear_protected_if_owned(&filter, generation);
    }
}
static GLOBAL_CAPTURE_FILTER: OnceLock<Arc<CaptureFilter>> = OnceLock::new();
static TEXT_POPUP: RwLock<Option<gpui::AnyWindowHandle>> = RwLock::new(None);
pub fn set_global_store(repository: Arc<TextRepository>, store: ArcStickerStore) {
    let _ = GLOBAL_REPOSITORY.set(repository);
    let _ = GLOBAL_STORE.set(store);
}
pub fn set_global_capture_filter(filter: Arc<CaptureFilter>) {
    let _ = GLOBAL_CAPTURE_FILTER.set(filter);
}
pub(crate) fn text_repository() -> Option<Arc<TextRepository>> {
    GLOBAL_REPOSITORY.get().cloned()
}
fn clear_popup() {
    if let Ok(mut slot) = TEXT_POPUP.write() {
        *slot = None;
    }
    super::super::set_text_escape_target(false);
}
pub fn dispatch_escape(cx: &mut App) -> bool {
    TEXT_POPUP
        .read()
        .ok()
        .and_then(|s| *s)
        .and_then(|h| {
            h.update(cx, |_, w, cx| {
                w.dispatch_keystroke(gpui::Keystroke::parse("escape").unwrap(), cx)
            })
            .ok()
        })
        .unwrap_or(false)
}
pub fn open_from_main(
    cx: &mut App,
    events: std::sync::mpsc::Sender<StickerWindowEvent>,
) -> anyhow::Result<()> {
    let repository =
        text_repository().ok_or_else(|| anyhow::anyhow!("Text backend unavailable"))?;
    TextPickerWindow::toggle(cx, events, repository, None)
}
pub struct TextPickerWindow;
impl TextPickerWindow {
    pub fn toggle(
        cx: &mut App,
        events: std::sync::mpsc::Sender<StickerWindowEvent>,
        repository: Arc<TextRepository>,
        destination: Option<u64>,
    ) -> anyhow::Result<()> {
        if TEXT_POPUP.read().ok().and_then(|s| *s).is_some() && dispatch_escape(cx) {
            return Ok(());
        }
        let filter = GLOBAL_CAPTURE_FILTER
            .get()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Capture backend unavailable"))?;
        let store = GLOBAL_STORE
            .get()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Sticker backend unavailable"))?;
        // Physical window geometry is an OS boundary; internal layout uses rem helpers.
        let bounds = Bounds::centered(None, size(px(300.), px(400.)), cx);
        let handle = cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_background: WindowBackgroundAppearance::Transparent,
                is_resizable: true,
                window_min_size: Some(size(px(300.), px(400.))),
                kind: WindowKind::Floating,
                titlebar: None,
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| {
                    TextPicker::new(repository, store, filter, events, destination, window, cx)
                });
                cx.new(|cx| Root::new(view, window, cx).bg(gpui::transparent_black()))
            },
        )?;
        if let Ok(mut slot) = TEXT_POPUP.write() {
            *slot = Some(handle.into());
        }
        // Reassert after native creation: the hotkey can arrive while another
        // application is active, before the new window has been presented.
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, _| super::platform::raise(window));
        });
        super::super::set_text_escape_target(true);
        Ok(())
    }
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SourceFilter {
    #[default]
    All,
    Text,
    Clipboard,
}
impl SourceFilter {
    fn includes(self, id: TextEntryId) -> bool {
        matches!(
            (self, id),
            (Self::All, _)
                | (Self::Text, TextEntryId::Sticker(_))
                | (Self::Clipboard, TextEntryId::Clipboard(_))
        )
    }
}

// Display-only recency; never changes the stored timestamp or clipboard payload.
fn recency_label(timestamp: i64, now: i64) -> String {
    let seconds = now.saturating_sub(timestamp).max(0) / 1000;
    match seconds {
        0..60 => "Just now".into(),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86400 => format!("{}h ago", seconds / 3600),
        86400..172800 => "Yesterday".into(),
        _ => format!("{}d ago", seconds / 86400),
    }
}

struct TextPicker {
    repository: Arc<TextRepository>,
    store: ArcStickerStore,
    filter: Arc<CaptureFilter>,
    events: std::sync::mpsc::Sender<StickerWindowEvent>,
    rows: Vec<TextEntry>,
    filtered: Vec<usize>,
    selected: usize,
    query: Entity<InputState>,
    scroll: UniformListScrollHandle,
    destination: Option<u64>,
    busy: bool,
    /// True while a paste-into-destination is in flight. The activation
    /// observer must ignore deactivation events during this window: switching
    /// the OS foreground to the destination is required to land the
    /// simulated Ctrl+V, and the picker naturally loses focus as a side
    /// effect. Without the gate the observer would self-destruct the window
    /// (or race with the explicit removal) before clipboard write and paste
    /// dispatch complete.
    delivering: bool,
    status: String,
    capture_enabled: bool,
    source: SourceFilter,
    loading: bool,
    preferred_selection: Option<TextEntryId>,
    _query: Subscription,
    _activation: Subscription,
}
impl Drop for TextPicker {
    fn drop(&mut self) {
        clear_popup();
    }
}
impl TextPicker {
    fn new(
        repository: Arc<TextRepository>,
        store: ArcStickerStore,
        filter: Arc<CaptureFilter>,
        events: std::sync::mpsc::Sender<StickerWindowEvent>,
        destination: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search texts ..."));
        let subscription = cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.refilter(cx);
                this.selected = 0;
                this.scroll.scroll_to_item(0, ScrollStrategy::Nearest);
                this.status.clear();
                cx.notify();
            }
        });
        let activation = cx.observe_window_activation(window, |this, window, _| {
            // Hold off on the deactivation auto-close while a paste is in
            // flight: the foreground switch back to the destination ditches
            // the picker's focus as a side effect, and the observer must not
            // double up on the explicit removal that `deliver` schedules.
            if this.delivering || window.is_window_active() {
                return;
            }
            clear_popup();
            window.remove_window();
        });
        let this = Self {
            repository,
            store,
            filter,
            events,
            rows: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            query: query.clone(),
            scroll: UniformListScrollHandle::new(),
            destination,
            busy: false,
            delivering: false,
            status: String::new(),
            capture_enabled: false,
            source: SourceFilter::All,
            loading: true,
            preferred_selection: None,
            _query: subscription,
            _activation: activation,
        };
        query.update(cx, |input, cx| input.focus(window, cx));
        cx.spawn(async move |entity, cx| {
            let mut last_version = -1;
            loop {
                let repository = match entity
                    .update(cx, |this, _| (!this.busy).then(|| this.repository.clone()))
                {
                    Ok(repository) => repository,
                    Err(_) => break,
                };
                let Some(repository) = repository else {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(400))
                        .await;
                    continue;
                };
                let version = repository.data_version().await.unwrap_or(-1);
                if version == last_version && version >= 0 {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(400))
                        .await;
                    continue;
                }
                let result = repository.snapshot(TextSources::new(true, true), "").await;
                if result.is_ok() {
                    last_version = version;
                }
                let settings = repository.settings().await;
                if entity
                    .update(cx, |this, cx| {
                        if this.busy {
                            last_version = -1;
                            return;
                        }
                        let id = this
                            .preferred_selection
                            .take()
                            .or_else(|| this.selected_entry().map(|e| e.id()));
                        this.loading = false;
                        match result {
                            Ok(rows) => {
                                this.rows = rows;
                                this.refilter(cx);
                                if let Some(id) = id
                                    && let Some(ix) = this
                                        .filtered
                                        .iter()
                                        .position(|ix| this.rows[*ix].id() == id)
                                {
                                    this.selected = ix;
                                }
                            }
                            Err(_) => this.status = "Could not load texts. Retrying.".into(),
                        };
                        if let Ok(settings) = settings {
                            this.capture_enabled = settings.capture_enabled;
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(400))
                    .await;
            }
        })
        .detach();
        this
    }
    fn refilter(&mut self, cx: &App) {
        self.filtered = filter_indices(&self.rows, self.query.read(cx).value().as_ref())
            .into_iter()
            .filter(|ix| self.source.includes(self.rows[*ix].id()))
            .collect();
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }
    fn selected_entry(&self) -> Option<TextEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|ix| self.rows.get(*ix))
            .cloned()
    }
    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.filtered.len().saturating_sub(1));
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }
    fn save(&mut self, encrypted: bool, edit: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if encrypted
            && let Some(entry) = self.selected_entry()
            && !entry.is_encrypted()
        {
            let weak = cx.weak_entity();
            let initial = if matches!(entry.id(), TextEntryId::Sticker(_)) {
                entry.title()
            } else {
                ""
            };
            super::title::prompt(
                window,
                cx,
                true,
                initial,
                Box::new(move |title, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |this, cx| {
                            this.save_titled(encrypted, edit, Some(title), window, cx)
                        });
                    }
                }),
            );
            return;
        }
        self.save_titled(encrypted, edit, None, window, cx);
    }
    fn save_titled(
        &mut self,
        encrypted: bool,
        edit: bool,
        title: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        let Some(entry) = self.selected_entry() else {
            return;
        };
        let repo = self.repository.clone();
        let store = self.store.clone();
        let events = self.events.clone();
        let weak = cx.entity().downgrade();
        self.busy = true;
        cx.notify();
        window
            .spawn(cx, async move |cx| {
                let probe = repo.clone();
                if encrypted && !entry.is_encrypted() {
                    let configured = cx
                        .background_executor()
                        .spawn(async move { futures::executor::block_on(probe.master_key()) })
                        .await;
                    if !matches!(configured, Ok(Some(_))) {
                        if let Some(entity) = weak.upgrade() {
                            let _ = entity.update_in(cx, |this, window, cx| {
                                this.busy = false;
                                super::password::settings(window, cx, repo);
                                cx.notify();
                            });
                        }
                        return;
                    }
                }
                let operation = repo.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        futures::executor::block_on(async {
                            match entry.id() {
                                TextEntryId::Clipboard(_) => {
                                    operation
                                        .promote_titled(&entry, encrypted, title.as_deref())
                                        .await
                                }
                                TextEntryId::Sticker(id) => {
                                    if encrypted && !entry.is_encrypted() {
                                        operation
                                            .set_encryption_titled(
                                                &entry,
                                                true,
                                                None,
                                                title.as_deref(),
                                            )
                                            .await?;
                                    }
                                    Ok(id)
                                }
                            }
                        })
                    })
                    .await;
                match result {
                    Ok(id) => {
                        if encrypted {
                            let _ = cx.update(|_, cx| {
                                StickerWindow::try_close(id, cx);
                            });
                        }
                        let _ = events.send(StickerWindowEvent::Created { id });
                        if edit {
                            let _ = cx.update(|_, cx| {
                                let events = events.clone();
                                let store = store.clone();
                                cx.spawn(async move |cx| {
                                    let _ = StickerWindow::open_async_with_options(
                                        cx, events, store, id, true,
                                    )
                                    .await;
                                })
                                .detach();
                            });
                        }
                        if let Some(entity) = weak.upgrade() {
                            let _ = entity.update_in(cx, |this, window, cx| {
                                this.busy = false;
                                this.preferred_selection = Some(TextEntryId::Sticker(id));
                                this.status = if encrypted {
                                    "Saved encrypted text"
                                } else {
                                    "Saved text"
                                }
                                .into();
                                if edit {
                                    clear_popup();
                                    window.remove_window();
                                }
                                cx.notify();
                            });
                        }
                    }
                    Err(_) => {
                        if let Some(entity) = weak.upgrade() {
                            let _ = entity.update_in(cx, |this, _, cx| {
                                this.busy = false;
                                this.status =
                                    "Text changed or could not be saved. Select it again.".into();
                                cx.notify();
                            });
                        }
                    }
                }
            })
            .detach();
    }
    fn use_selected(&mut self, copy_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(entry) = self.selected_entry() else {
            return;
        };
        if entry.is_encrypted() {
            let weak = cx.entity().downgrade();
            super::password::unlock(
                window,
                cx,
                Box::new(move |password, window, cx| {
                    if let Some(entity) = weak.upgrade() {
                        entity.update(cx, |this, cx| {
                            this.deliver(entry, copy_only, Some(password), window, cx)
                        });
                    }
                }),
            );
        } else {
            self.deliver(entry, copy_only, None, window, cx);
        }
    }
    fn deliver(
        &mut self,
        entry: TextEntry,
        copy_only: bool,
        password: Option<Zeroizing<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let repo = self.repository.clone();
        let filter = self.filter.clone();
        let destination = self.destination;
        let weak = cx.entity().downgrade();
        self.busy = true;
        self.delivering = true;
        cx.notify();
        // Paste switches foreground windows. The authorized action is app-owned so
        // closing this transient picker cannot cancel delivery or revive it on failure.
        let handle = window.window_handle();
        // Switch the OS foreground to the destination *while the picker is
        // still alive*. Cross-process `SetForegroundWindow` is gated by
        // Windows' foreground lock, and rustickers only owns that right
        // while one of its windows is the foreground window. Running it
        // asynchronously (after the picker is gone) silently fails for
        // external destinations such as Notepad because User32 routes the
        // call through the new (or null) foreground process until the lock
        // expires -- which is why prior versions pasted into rustickers-
        // owned windows fine but failed against any other app.
        let foreground_done = if !copy_only {
            match destination {
                None => false,
                Some(destination) => {
                    if let Err(err) =
                        crate::native::clipboard::paste::foreground(Some(destination))
                    {
                        let _ = weak.update(cx, |this, cx| {
                            this.busy = false;
                            this.delivering = false;
                            this.status = format!("Could not use text. {err}");
                            cx.notify();
                        });
                        return;
                    }
                    true
                }
            }
        } else {
            false
        };
        if !copy_only {
            clear_popup();
            window.remove_window();
        }
        App::spawn(cx, async move |cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    futures::executor::block_on(async {
                        let text = repo
                            .resolve(&entry, password.as_ref().map(|p| p.as_str()))
                            .await?;
                        dispatch_content(
                            &filter,
                            &text,
                            destination,
                            copy_only,
                            entry.is_encrypted(),
                            foreground_done,
                        )?;
                        let _ = repo
                            .mark_used(entry.id(), crate::utils::time::now_unix_millis())
                            .await;
                        anyhow::Ok(())
                    })
                })
                .await;
            let _ = handle.update(cx, |_, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.busy = false;
                    this.delivering = false;
                    if result.is_ok() {
                        clear_popup();
                        window.remove_window();
                    } else {
                        this.status =
                            "Could not use text. Check the password or paste destination.".into();
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || window.has_active_dialog(cx) {
            return;
        }
        let Some(entry) = self.selected_entry() else {
            return;
        };
        let weak = cx.entity().downgrade();
        if matches!(entry.id(), TextEntryId::Sticker(_)) {
            window.open_alert_dialog(cx, move |dialog, _, _| {
                let weak = weak.clone();
                let entry = entry.clone();
                dialog
                    .title("Delete saved text?")
                    .child("This permanently deletes the text sticker.")
                    .on_ok(move |_, window, cx| {
                        if let Some(entity) = weak.upgrade() {
                            entity.update(cx, |this, cx| {
                                this.delete_entry(entry.clone(), window, cx)
                            });
                        }
                        true
                    })
            });
        } else {
            self.delete_entry(entry, window, cx);
        }
    }
    fn delete_entry(&mut self, entry: TextEntry, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let repo = self.repository.clone();
        let events = self.events.clone();
        let id = entry.id();
        let entity = cx.entity().downgrade();
        self.busy = true;
        window
            .spawn(cx, async move |cx| {
                let result = repo.delete(id).await;
                if let TextEntryId::Sticker(id) = id {
                    let _ = events.send(StickerWindowEvent::Created { id });
                    let _ = cx.update(|_, cx| StickerWindow::try_close(id, cx));
                }
                if let Some(entity) = entity.upgrade() {
                    let _ = entity.update_in(cx, |this, _, cx| {
                        this.busy = false;
                        if result.is_err() {
                            this.status = "Could not delete text.".into();
                        }
                        cx.notify();
                    });
                }
            })
            .detach();
    }
    fn toggle_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        cx.notify();
        let repo = self.repository.clone();
        let entity = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = async {
                    let mut settings = repo.settings().await?;
                    settings.capture_enabled = !settings.capture_enabled;
                    repo.update_settings(&settings).await?;
                    anyhow::Ok(settings.capture_enabled)
                }
                .await;
                if let Some(entity) = entity.upgrade() {
                    let _ = entity.update_in(cx, |this, _, cx| {
                        this.busy = false;
                        match result {
                            Ok(enabled) => {
                                this.capture_enabled = enabled;
                                this.status.clear();
                            }
                            Err(_) => this.status = "Could not update collection settings.".into(),
                        };
                        cx.notify();
                    });
                }
            })
            .detach();
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let command = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
        if window.has_active_dialog(cx) || self.busy {
            return;
        }
        // Menus consume Escape first; toolbar focus must not strand the utility.
        if key != "escape" && !self.query.focus_handle(cx).contains_focused(window, cx) {
            return;
        }
        match key {
            "escape" => {
                if !self.busy {
                    clear_popup();
                    window.remove_window();
                }
            }
            "up" => self.move_selection(-1, cx),
            "down" => self.move_selection(1, cx),
            "pageup" => self.move_selection(-3, cx),
            "pagedown" => self.move_selection(3, cx),
            "enter" => self.use_selected(command || self.destination.is_none(), window, cx),
            "f2" if command => self.rename(window, cx),
            "f2" => self.save(false, true, window, cx),
            "delete" if self.query.read(cx).value().is_empty() => self.delete(window, cx),
            "r" if command => self.save(false, false, window, cx),
            "l" if command => self.save(true, false, window, cx),
            _ => return,
        };
        cx.stop_propagation();
    }
}
impl Focusable for TextPicker {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.query.focus_handle(cx)
    }
}
impl TextPicker {
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .justify_between()
            .pb_2()
            .gap_1()
            .window_control_area(WindowControlArea::Drag)
            .child(
                div().flex_1().min_w_0().occlude().child(
                    Input::new(&self.query)
                        .prefix(Icon::new(IconName::Search))
                        .cleanable(true)
                        .border_0()
                        .focus_bordered(false)
                        .w_full()
                        .max_w_64()
                        .disabled(self.busy),
                ),
            )
            .child(
                h_flex()
                    .child(
                        Button::new("master-password")
                            .icon(IconName::LockClosed)
                            .ghost()
                            .occlude()
                            .tooltip("Master password…")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                super::password::settings(window, cx, this.repository.clone())
                            })),
                    )
                    .child(
                        Button::new("close-texts")
                            .icon(IconName::Close)
                            .ghost()
                            .occlude()
                            .tooltip("Close · Esc")
                            .disabled(self.busy)
                            .on_click(|_, window, _| {
                                clear_popup();
                                window.remove_window();
                            }),
                    ),
            )
    }

    fn render_filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .px_2()
            .pb_2()
            .gap_1()
            .flex_shrink_0()
            .children(
                [
                    (SourceFilter::All, "all-texts", "All"),
                    (SourceFilter::Text, "saved-texts", "Text"),
                    (SourceFilter::Clipboard, "clipboard-texts", "Clipboard"),
                ]
                .into_iter()
                .map(|(source, id, label)| {
                    Button::new(id)
                        .label(label)
                        .ghost()
                        .small()
                        .selected(self.source == source)
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.source = source;
                            this.selected = 0;
                            this.refilter(cx);
                            this.scroll.scroll_to_item(0, ScrollStrategy::Nearest);
                            this.status.clear();
                            this.query.update(cx, |query, cx| query.focus(window, cx));
                            cx.notify();
                        }))
                }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.filtered.len().to_string()),
            )
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let entry = &self.rows[self.filtered[ix]];
        let id = entry.id();
        let encrypted = entry.is_encrypted();
        let saved = matches!(id, TextEntryId::Sticker(_));
        let selected = ix == self.selected;
        v_flex()
            .id(gpui::SharedString::from(id.to_string()))
            .group("text-row")
            .w_full()
            .flex_shrink_0()
            .gap_0p5()
            .px_3()
            .py_2()
            .bg(if selected {
                cx.theme().list_active.alpha(0.3)
            } else {
                cx.theme().transparent
            })
            .when(!selected && !self.busy, |row| {
                row.hover(|row| row.bg(cx.theme().list_hover))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if this.busy || window.has_active_dialog(cx) {
                        return;
                    }
                    if this.select_id(id) {
                        this.status.clear();
                        this.query.update(cx, |query, cx| query.focus(window, cx));
                        if event.click_count == 2 {
                            this.save(false, true, window, cx);
                        }
                        cx.notify();
                    }
                }),
            )
            .child(
                div().text_sm().truncate().child(
                    entry
                        .preview()
                        .replace(['\r', '\n'], "↩")
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .trim()
                        .to_owned(),
                ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_shrink_0()
                    .items_center()
                    .child(
                        Icon::new(if encrypted {
                            IconName::LockClosed
                        } else if saved {
                            IconName::DocumentText
                        } else {
                            IconName::Copy
                        })
                        .size_4()
                        .flex_shrink_0()
                        .text_color(if encrypted {
                            cx.theme().warning
                        } else if saved {
                            cx.theme().primary
                        } else {
                            cx.theme().muted_foreground
                        }),
                    )
                    .child(self.render_row_actions(id, saved, encrypted, cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(recency_label(
                                entry.activity_at(),
                                crate::utils::time::now_unix_millis(),
                            )),
                    ),
            )
            .into_any_element()
    }

    fn select_id(&mut self, id: TextEntryId) -> bool {
        let Some(ix) = self
            .filtered
            .iter()
            .position(|ix| self.rows[*ix].id() == id)
        else {
            return false;
        };
        self.selected = ix;
        true
    }

    fn render_row_actions(
        &self,
        id: TextEntryId,
        saved: bool,
        encrypted: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .gap_1()
            .invisible()
            .group_hover("text-row", |actions| actions.visible())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(
                [
                    (
                        "rename",
                        IconName::DocumentText,
                        "Rename… · Ctrl+F2",
                        !saved,
                    ),
                    ("edit", IconName::Adjustments, "Edit… · F2", false),
                    ("save", IconName::Pin, "Save as sticker · Ctrl+R", saved),
                    ("lock", IconName::LockClosed, "Lock… · Ctrl+L", encrypted),
                    ("delete", IconName::Delete, "Delete · Delete", false),
                ]
                .into_iter()
                .filter(|(action, _, _, disabled)| !*disabled || *action == "lock")
                .map(|(action, icon, hint, disabled)| {
                    Button::new(gpui::SharedString::from(format!("{action}-{id}")))
                        .icon(icon)
                        .ghost()
                        .small()
                        .tooltip(hint)
                        .disabled(self.busy || disabled)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            if this.busy || !this.select_id(id) {
                                return;
                            }
                            match action {
                                "rename" => this.rename(window, cx),
                                "edit" => this.save(false, true, window, cx),
                                "save" => this.save(false, false, window, cx),
                                "lock" => this.save(true, false, window, cx),
                                "delete" => this.delete(window, cx),
                                _ => unreachable!(),
                            }
                        }))
                }),
            )
    }

    fn rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(entry) = self.selected_entry() else {
            return;
        };
        if !matches!(entry.id(), TextEntryId::Sticker(_)) {
            return;
        }
        let repo = self.repository.clone();
        let events = self.events.clone();
        let weak = cx.weak_entity();
        let initial = entry.title().to_owned();
        super::title::prompt(
            window,
            cx,
            false,
            &initial,
            Box::new(move |title, window, cx| {
                window
                    .spawn(cx, async move |cx| {
                        let result = repo.update_title(entry.id(), &title).await;
                        if result.is_ok()
                            && let TextEntryId::Sticker(id) = entry.id()
                        {
                            let _ = events.send(StickerWindowEvent::TitleChanged { id, title });
                        }
                        let _ = weak.update_in(cx, |this, _, cx| {
                            this.status = if result.is_ok() {
                                "Title updated"
                            } else {
                                "Could not update title. Try again."
                            }
                            .into();
                            cx.notify();
                        });
                    })
                    .detach();
            }),
        );
    }
    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .gap_1()
            .pt_1()
            .child(
                div()
                    .px_3()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .truncate()
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.busy {
                        "Working…".to_owned()
                    } else if !self.status.is_empty() {
                        self.status.clone()
                    } else {
                        if self.destination.is_some() {
                            "↑↓ select · Enter paste · F2 edit"
                        } else {
                            "↑↓ select · Enter copy · F2 edit"
                        }
                        .into()
                    }),
            )
            .child(
                Button::new("capture-toggle")
                    .label(if self.capture_enabled {
                        "Collecting"
                    } else {
                        "Paused"
                    })
                    .icon(if self.capture_enabled {
                        IconName::Pause
                    } else {
                        IconName::Play
                    })
                    .ghost()
                    .small()
                    .disabled(self.busy)
                    .tooltip(if self.capture_enabled {
                        "Pause clipboard collection"
                    } else {
                        "Enable clipboard collection. Copied secrets can enter history."
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_capture(window, cx))),
            )
    }
}
impl Render for TextPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let count = self.filtered.len();
        v_flex().size_full().relative().rounded_lg().overflow_hidden()
            .bg(cx.theme().background.opacity(crate::native::theme::WINDOW_SURFACE_OPACITY)).text_color(cx.theme().foreground)
            .track_focus(&self.query.focus_handle(cx)).on_key_down(cx.listener(Self::key))
            .child(self.render_header(cx))
            .child(self.render_filters(cx))
            .child(div().flex_1().min_h_0().w_full().overflow_hidden()
                .when(count > 0, |body| body.child(uniform_list("texts-list", count, move |range, _, cx| {
                    view.update(cx, |this, cx| range.map(|ix| this.render_row(ix, cx)).collect::<Vec<_>>())
                }).size_full().track_scroll(&self.scroll)))
                .when(count == 0, |body| body.child(v_flex().size_full().items_center().justify_center().gap_2().px_4()
                    .child(Icon::new(IconName::DocumentText).size_6().text_color(cx.theme().muted_foreground))
                    .child(div().text_sm().child(if self.loading { "Loading texts…" } else if !self.query.read(cx).value().is_empty() { "No matching texts" } else if self.source == SourceFilter::Text { "No saved texts yet" } else if self.source == SourceFilter::Clipboard { "No clipboard captures yet" } else { "Your text, within reach" }))
                    .child(div().text_xs().text_center().text_color(cx.theme().muted_foreground)
                        .child(if !self.query.read(cx).value().is_empty() { "Try another search or clear the filter." } else if self.source == SourceFilter::Text { "Save a capture with its pin button or Ctrl+R to keep it." } else if self.capture_enabled { "Copied text will appear here. Saved stickers are always kept." } else { "Enable collection below, or create a text sticker." })))))
            .child(self.render_footer(cx))
    }
}

fn dispatch_content(
    filter: &Arc<CaptureFilter>,
    content: &str,
    destination: Option<u64>,
    copy_only: bool,
    secret: bool,
    foreground_done: bool,
) -> anyhow::Result<()> {
    if secret {
        anyhow::ensure!(
            cfg!(target_os = "windows"),
            "Ownership-aware protected clipboard expiry is only available on Windows"
        );
    }
    if !copy_only {
        crate::native::clipboard::paste::wait_for_modifiers()?;
        if !foreground_done {
            foreground(destination)?;
        }
    }
    let generation =
        crate::native::clipboard::watcher::write_clipboard_text_owned(filter, content)?;
    if secret {
        *OWNED_SECRET.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((filter.clone(), generation));
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

#[cfg(test)]
mod picker_tests {
    use super::*;
    #[test]
    fn source_filters_keep_typed_identity_separate() {
        assert!(SourceFilter::All.includes(TextEntryId::Sticker(1)));
        assert!(SourceFilter::All.includes(TextEntryId::Clipboard(1)));
        assert!(SourceFilter::Text.includes(TextEntryId::Sticker(1)));
        assert!(!SourceFilter::Text.includes(TextEntryId::Clipboard(1)));
        assert!(SourceFilter::Clipboard.includes(TextEntryId::Clipboard(1)));
        assert!(!SourceFilter::Clipboard.includes(TextEntryId::Sticker(1)));
    }
    #[test]
    fn relative_time_handles_future_and_boundaries() {
        assert_eq!(recency_label(10_000, 0), "Just now");
        assert_eq!(recency_label(0, 60_000), "1m ago");
        assert_eq!(recency_label(0, 3_600_000), "1h ago");
        assert_eq!(recency_label(0, 86_400_000), "Yesterday");
        assert_eq!(recency_label(0, 172_800_000), "2d ago");
    }
}
