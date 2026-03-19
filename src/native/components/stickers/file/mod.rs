mod audio;
mod editor;
mod preview;
mod summary;
mod utils;
mod watcher;

use futures::channel::mpsc as async_mpsc;
use gpui::{Context, Entity, Rgba, Window, div, prelude::*, px, rgba};
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
            cx.observe_keystrokes(move |this, event, window, cx| {
                if !this.is_persisted() && event.keystroke.key == "escape" {
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
            .absolute()
            .top_0()
            .right(px(68.0))
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
            .absolute()
            .top_0()
            .right_20()
            .disabled(self.refreshing)
            .bg(rgba(0x000000))
            .border_0()
            .cursor_pointer()
            .occlude()
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some(FilePreview::WebView(web)) = &this.preview {
                    let _ = web.update(cx, |web, cx| web.reload(cx));
                    this.spawn_refresh_summaries(window, cx);
                    return;
                }
                this.spawn_refresh_preview(window, cx);
                this.spawn_refresh_summaries(window, cx);
            }))
            .into_any_element()
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
}

impl Render for FileSticker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(14.0));

        self.ensure_watch_loop(window, cx);

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
                div().h_full().flex_shrink().overflow_hidden().child(
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
            .when(!self.is_persisted() && window.is_window_hovered(), |view| {
                view.child(self.pin_btn(cx))
            })
            .when(window.is_window_hovered(), |view| {
                view.child(self.refresh_btn(cx))
            })
    }
}
