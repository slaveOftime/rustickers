mod audio;
mod editor;
mod preview;
mod summary;
mod utils;
mod watcher;

use futures::channel::mpsc as async_mpsc;
use gpui::{Context, Entity, Rgba, Window, div, prelude::*, px, rgba};
use gpui_component::h_flex;
use gpui_component::{
    Disableable, alert::Alert, button::Button, scroll::ScrollableElement, v_flex,
};
use notify::RecommendedWatcher;
use preview::FilePreview;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use summary::FileSummary;
use url::Url;

use crate::model::content::FileStickerContent;
use crate::model::sticker::{StickerColor, StickerDetail, StickerState, StickerType};
use crate::native::components::IconName;
use crate::native::windows::StickerWindowEvent;
use crate::native::windows::sticker::StickerWindow;
use crate::storage::ArcStickerStore;

use super::content_lock::{LockActions, LockForm, LockedContent, UnlockActions};

pub struct FileSticker {
    id: i64,
    color: StickerColor,
    store: ArcStickerStore,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    source_paths: Vec<String>,
    summaries: Vec<FileSummary>,
    preview: Option<FilePreview>,
    preview_editor: Option<Entity<gpui_component::input::InputState>>,
    refreshing: bool,
    pining: bool,
    error: Option<String>,
    watcher: Option<RecommendedWatcher>,
    watch_events_rx: Option<async_mpsc::UnboundedReceiver<()>>,
    watch_loop_started: bool,
    watch_pending: Arc<AtomicBool>,
    watch_stop: Arc<AtomicBool>,
    locked_content: Option<LockedContent>,
    locked_path: Option<std::path::PathBuf>,
    content_visible: bool,
    locking: bool,
    lock_busy: bool,
    lock_form: LockForm,
}

impl FileSticker {
    pub fn default_window_size_for_sources(sources: &[&str]) -> gpui::Size<i32> {
        if sources.is_empty() {
            return <Self as super::Sticker>::default_window_size();
        }

        if sources.len() > 1 {
            let row_bonus = (sources.len().saturating_sub(1) as i32).clamp(0, 14) * 12;
            return gpui::size(300, (380 + row_bonus).clamp(380, 560));
        }

        let source = &sources[0];
        let ext = if crate::utils::url::is_url(source) {
            Url::parse(source).ok().and_then(|url| {
                Path::new(url.path())
                    .extension()
                    .and_then(|raw_ext| raw_ext.to_str())
                    .map(|raw_ext| raw_ext.to_ascii_lowercase())
            })
        } else {
            let path = Path::new(source);
            if path.is_dir() {
                return gpui::size(300, 200);
            }
            path.extension()
                .and_then(|raw_ext| raw_ext.to_str())
                .map(|raw_ext| raw_ext.to_ascii_lowercase())
        };

        if let Some(ext) = ext.as_deref() {
            if crate::utils::file::is_image_ext(ext) {
                return gpui::size(480, 480);
            }
            if crate::utils::file::is_web_doc_ext(ext) || crate::utils::file::is_video_ext(ext) {
                return gpui::size(760, 640);
            }
            if crate::utils::file::is_audio_ext(ext) {
                return gpui::size(340, 160);
            }
            if crate::utils::file::is_markdown_ext(ext) {
                return gpui::size(640, 760);
            }
            if crate::utils::file::is_code_ext(ext) {
                return gpui::size(640, 760);
            }
            if crate::utils::file::is_text_ext(ext) {
                return gpui::size(540, 640);
            }
        }
        gpui::size(400, 200)
    }

    pub fn new(
        id: i64,
        color: StickerColor,
        store: ArcStickerStore,
        content: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    ) -> Self {
        if id <= 0 {
            window.activate_window();
            let own_window_id = window.window_handle().window_id();
            cx.observe_keystrokes(move |this, event, window, cx| {
                if window.window_handle().window_id() == own_window_id
                    && !this.is_persisted()
                    && !this.lock_interaction_active()
                    && event.keystroke.key == "escape"
                {
                    if !StickerWindow::try_close(id, cx) {
                        window.remove_window();
                    }
                    this.stop_audio();
                }
            })
            .detach();
        }

        let parsed = FileStickerContent::from_json_or_raw(content);
        let source_paths = parsed.files;
        let mut summaries: Vec<FileSummary> = source_paths
            .iter()
            .map(|raw_path| FileSummary::new(raw_path))
            .collect();
        summaries.sort_by(|a, b| a.name.cmp(&b.name));
        let initial_title = if summaries.len() == 1 {
            summaries[0].name.clone()
        } else {
            "Private file".to_string()
        };
        let lock_form = LockForm::new(initial_title, "Password to unlock this file", window, cx);

        let mut this = Self {
            id,
            color,
            store,
            sticker_events_tx,
            source_paths,
            summaries,
            preview: None,
            preview_editor: None,
            refreshing: false,
            pining: false,
            error: None,
            watcher: None,
            watch_events_rx: None,
            watch_loop_started: false,
            watch_pending: Arc::new(AtomicBool::new(false)),
            watch_stop: Arc::new(AtomicBool::new(false)),
            locked_content: None,
            locked_path: None,
            content_visible: true,
            locking: false,
            lock_busy: false,
            lock_form,
        };

        if this.is_persisted() {
            this.init_file_watcher();
        }
        this.spawn_refresh_preview(window, cx);
        this.spawn_refresh_summaries(window, cx);
        this
    }

    fn is_persisted(&self) -> bool {
        self.id > 0
    }

    fn title(&self) -> String {
        if self.summaries.len() == 1 {
            self.summaries[0].name.clone()
        } else {
            format!("{} files", self.summaries.len())
        }
    }

    fn pin(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.is_persisted() || self.pining {
            return;
        }
        self.pining = true;
        self.error = None;
        cx.notify();

        let title = self.title();
        let content = FileStickerContent {
            files: self
                .summaries
                .iter()
                .map(|item| item.path.clone())
                .collect(),
        }
        .to_json();
        let bounds = window.bounds();
        let detail = StickerDetail {
            id: 0,
            title,
            state: StickerState::Open,
            left: bounds.left().to_f64() as i32,
            top: bounds.top().to_f64() as i32,
            width: bounds.size.width.to_f64() as i32,
            height: bounds.size.height.to_f64() as i32,
            top_most: false,
            color: self.color,
            sticker_type: StickerType::File,
            content,
            created_at: 0,
            updated_at: 0,
            display_id: None,
            display_uuid: None,
            virtual_desktop_id: None,
            native_left: None,
            native_top: None,
            native_width: None,
            native_height: None,
            preferred_display_uuid: None,
            placements: Vec::new(),
        };

        let store = self.store.clone();
        let sticker_events_tx = self.sticker_events_tx.clone();
        let original_id = self.id;
        cx.spawn(
            async move |entity, cx| match store.insert_sticker(detail).await {
                Ok(id) => {
                    StickerWindow::swap_open_sticker_id(original_id, id);
                    let _ = sticker_events_tx.send(StickerWindowEvent::Created { id });
                    let _ = entity.update(cx, |this, cx| {
                        this.id = id;
                        this.pining = false;
                        this.init_file_watcher();
                        cx.notify();
                    });
                }
                Err(err) => {
                    let _ = entity.update(cx, |this, cx| {
                        this.pining = false;
                        this.error = Some(format!("Failed to save file sticker: {err:#}"));
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn pin_btn(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        Button::new("stick")
            .icon(IconName::Pin)
            .disabled(self.pining)
            .bg(rgba(0x000000))
            .border_0()
            .cursor_pointer()
            .occlude()
            .on_click(cx.listener(|this, _, window, cx| {
                this.pin(window, cx);
            }))
            .into_any_element()
    }

    fn refresh_btn(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        Button::new("refresh")
            .icon(IconName::Refresh)
            .disabled(self.refreshing)
            .bg(rgba(0x000000))
            .border_0()
            .cursor_pointer()
            .occlude()
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some(FilePreview::WebView(web)) = &this.preview {
                    web.update(cx, |web, cx| web.reload(cx));
                    this.spawn_refresh_summaries(window, cx);
                    return;
                }
                this.spawn_refresh_preview(window, cx);
                this.spawn_refresh_summaries(window, cx);
            }))
            .into_any_element()
    }

    fn can_lock(&self) -> bool {
        self.locked_content.is_some()
            || self
                .preview
                .as_ref()
                .and_then(FilePreview::lockable_content)
                .is_some()
    }

    fn lock_interaction_active(&self) -> bool {
        self.locking || !self.content_visible
    }

    fn begin_lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.lock_form.reset_for_lock(self.title(), window, cx);
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

    fn visible_lockable_content(&self, cx: &Context<Self>) -> Option<(std::path::PathBuf, String)> {
        if let Some(editor) = self.preview_editor.as_ref() {
            let path = self.preview.as_ref()?.editable_source()?.to_path_buf();
            return Some((path, editor.read(cx).value().to_string()));
        }
        self.preview
            .as_ref()?
            .lockable_content()
            .map(|(path, content)| (path.to_path_buf(), content.to_string()))
    }

    fn lock_new_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.lock_busy {
            return;
        }
        let Some((path, content)) = self.visible_lockable_content(cx) else {
            self.error = Some("Only editable text and markdown files can be locked".to_string());
            cx.notify();
            return;
        };
        let prepared = match self.lock_form.prepare_lock(&content, cx) {
            Ok(prepared) => prepared,
            Err(err) => {
                self.error = Some(err);
                cx.notify();
                return;
            }
        };
        match std::fs::write(&path, prepared.serialized.as_bytes()) {
            Ok(()) => {
                self.preview = Some(FilePreview::locked_text(
                    path.clone(),
                    prepared.locked.clone(),
                ));
                self.locked_content = Some(prepared.locked);
                self.locked_path = Some(path);
                self.content_visible = false;
                self.locking = false;
                self.preview_editor = None;
                self.lock_form.clear_passwords(window, cx);
                self.error = None;
            }
            Err(err) => self.error = Some(format!("Failed to lock file: {err}")),
        }
        cx.notify();
    }

    fn unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(locked), Some(path)) = (self.locked_content.clone(), self.locked_path.clone())
        else {
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
            if let Err(err) = std::fs::write(&path, updated.serialized) {
                self.error = Some(format!("Failed to save file title: {err}"));
                cx.notify();
                return;
            }
            self.locked_content = Some(updated.locked);
        }
        self.preview = Some(FilePreview::unlocked_text(path, prepared.content));
        self.content_visible = true;
        self.error = None;
        self.lock_form.clear_passwords(window, cx);
        cx.notify();
    }

    fn unlock_forever(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(locked), Some(path)) = (self.locked_content.clone(), self.locked_path.clone())
        else {
            return;
        };
        let prepared =
            match self
                .lock_form
                .prepare_unlock_forever(&locked, |_| locked.title.clone(), cx)
            {
                Ok(prepared) => prepared,
                Err(err) => {
                    self.error = Some(err);
                    cx.notify();
                    return;
                }
            };
        match std::fs::write(&path, prepared.content.as_bytes()) {
            Ok(()) => {
                self.preview = Some(FilePreview::unlocked_text(path, prepared.content));
                self.locked_content = None;
                self.locked_path = None;
                self.content_visible = true;
                self.preview_editor = None;
                self.lock_form.clear_passwords(window, cx);
                self.error = None;
            }
            Err(err) => self.error = Some(format!("Failed to remove file lock: {err}")),
        }
        cx.notify();
    }

    fn relock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_unlocked_content(window, cx);
    }

    fn hide_unlocked_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let (Some(path), Some(locked)) = (self.locked_path.clone(), self.locked_content.clone())
        {
            self.preview = Some(FilePreview::locked_text(path, locked));
        }
        self.content_visible = false;
        self.preview_editor = None;
        self.lock_form.clear_passwords(window, cx);
        self.lock_form.focus_password(window, cx);
        self.error = None;
        cx.notify();
    }
}

impl super::Sticker for FileSticker {
    fn id(&self) -> i64 {
        self.id
    }

    fn save_on_close(&mut self, _cx: &mut Context<Self>) -> bool {
        self.watch_stop.store(true, Ordering::Release);
        self.watcher = None;
        self.stop_audio();
        true
    }

    fn min_window_size() -> gpui::Size<i32> {
        gpui::size(24, 24)
    }

    fn default_window_size() -> gpui::Size<i32> {
        gpui::size(460, 320)
    }

    fn set_color(&mut self, color: StickerColor) {
        self.color = color;
    }

    fn disable_color_picker(&self) -> bool {
        match self.preview.as_ref() {
            Some(FilePreview::Audio { state, .. }) => state.is_playing,
            _ => false,
        }
    }

    fn use_default_bg(&self) -> bool {
        !matches!(self.preview, Some(FilePreview::WebView { .. }))
    }

    fn suppress_window_escape(&self) -> bool {
        self.preview_editor.is_some() || self.lock_interaction_active()
    }

    fn protected_content_visible(&self) -> bool {
        self.locked_content.is_some() && self.content_visible
    }

    fn relock_protected_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_unlocked_content(window, cx);
    }

    fn handle_lock_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.lock_busy || self.locking || !self.can_lock() {
            return false;
        }
        if self.locked_content.is_some() {
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
        let controls = gpui_component::h_flex()
            .flex_1()
            .gap_1()
            .child(div().flex_1());
        let controls = if self.is_persisted() {
            controls
        } else {
            controls.child(self.pin_btn(cx))
        };
        let controls = if self.can_lock() {
            controls.child(
                Button::new("file-content-lock")
                    .icon(if self.locked_content.is_some() && self.content_visible {
                        IconName::LockOpen
                    } else {
                        IconName::LockClosed
                    })
                    .tooltip(if self.locked_content.is_some() && self.content_visible {
                        "Lock file now (ctrl+l)"
                    } else if self.locked_content.is_some() {
                        "File is locked"
                    } else {
                        "Protect file with a password (ctrl+l)"
                    })
                    .disabled(self.lock_busy || self.locking)
                    .bg(rgba(0x000000))
                    .border_0()
                    .cursor_pointer()
                    .occlude()
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.locked_content.is_some() {
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
        } else {
            controls
        };
        Some(controls.child(self.refresh_btn(cx)).into_any_element())
    }

    fn footer_extension(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.content_visible || self.locking || self.locked_content.is_some() {
            return None;
        }
        if self.preview_editor.is_some() {
            return Some(
                h_flex()
                    .child(
                        Button::new("save-preview-file")
                            .label("Save (ctrl+s)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_edit(window, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .into_any_element(),
            );
        }

        None
    }

    fn is_footer_absoute(&self) -> bool {
        match self.preview.as_ref() {
            Some(FilePreview::WebView { .. }) => false,
            _ => true,
        }
    }
}

impl Render for FileSticker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(14.0));

        self.ensure_watch_loop(window, cx);

        if self.locking {
            return self.lock_form.locking_view(
                "file",
                "Lock file",
                self.error.as_deref(),
                self.lock_busy,
                LockActions {
                    cancel: Self::cancel_lock,
                    confirm: Self::lock_new_content,
                },
                cx,
            );
        }

        if !self.content_visible {
            return self.lock_form.locked_view(
                "file",
                self.error.as_deref(),
                self.lock_busy,
                UnlockActions {
                    cancel: Self::cancel_unlock,
                    unlock: Self::unlock,
                    unlock_forever: Self::unlock_forever,
                },
                cx,
            );
        }

        let bg_color = Rgba {
            a: 0.85,
            ..self.color.bg()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .when(
                matches!(self.preview, Some(FilePreview::WebView(_))),
                |view| {
                    view.bg(rgba(0x000000)).child(
                        div()
                            .bg(bg_color)
                            .py_1()
                            .pl_2()
                            .pr_16()
                            .text_sm()
                            .opacity(0.75)
                            .text_ellipsis_start()
                            .child(self.summaries[0].path.clone()),
                    )
                },
            )
            .child(
                div().h_full().flex_shrink(1.0).overflow_hidden().child(
                    v_flex()
                        .overflow_y_scrollbar()
                        .child(self.preview_view(window, cx)),
                ),
            )
            .when_some(self.error.as_ref(), |view, err| {
                view.child(
                    div()
                        .p_2()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .child(Alert::error("file-error", err.as_str())),
                )
            })
            .when(
                (self.preview.is_none() || self.summaries.is_empty()) && self.refreshing,
                |view| view.child(div().p_2().text_sm().opacity(0.85).child("Loading ...")),
            )
            .when(
                self.preview.is_some()
                    && !matches!(self.preview, Some(FilePreview::Audio { .. }))
                    && self.preview_editor.is_none()
                    && window.is_window_hovered(),
                |view| {
                    view.child(
                        div()
                            .bg(Rgba {
                                a: 0.95,
                                ..self.color.bg()
                            })
                            .shadow_md()
                            .border_t_1()
                            .border_dashed()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .child(
                                div()
                                    .max_h(px(180.0))
                                    .overflow_y_scrollbar()
                                    .child(self.summary_view()),
                            ),
                    )
                },
            )
            .into_any_element()
    }
}
