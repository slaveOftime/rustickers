mod audio;
mod editor;
mod preview;
mod summary;
mod utils;
mod watcher;

use futures::channel::mpsc as async_mpsc;
use gpui::{Context, Entity, Rgba, Window, WindowControlArea, div, prelude::*, px, rgba};
use gpui_component::{
    Disableable, alert::Alert, button::Button, scroll::ScrollableElement, v_flex,
};
use notify::RecommendedWatcher;
use preview::FilePreview;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use summary::FileSummary;

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
    sticking: bool,
    error: Option<String>,
    watcher: Option<RecommendedWatcher>,
    watch_events_rx: Option<async_mpsc::UnboundedReceiver<()>>,
    watch_loop_started: bool,
    watch_pending: Arc<AtomicBool>,
    watch_stop: Arc<AtomicBool>,
}

impl FileSticker {
    pub fn default_window_size_for_sources(sources: &[&str]) -> gpui::Size<i32> {
        preview::build_default_size(sources)
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
            .map(|raw_path| FileSummary::from_source(raw_path))
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
            sticking: false,
            error: None,
            watcher: None,
            watch_events_rx: None,
            watch_loop_started: false,
            watch_pending: Arc::new(AtomicBool::new(false)),
            watch_stop: Arc::new(AtomicBool::new(false)),
        };

        this.init_file_watcher();
        this.spawn_refresh_preview(window, cx);
        this.spawn_refresh_summaries(window, cx);
        this
    }

    fn is_persisted(&self) -> bool {
        self.id > 0
    }

    fn stick_title(&self) -> String {
        if self.summaries.len() == 1 {
            self.summaries[0].name.clone()
        } else {
            format!("{} files", self.summaries.len())
        }
    }

    fn stick_controls(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        Button::new("stick")
            .icon(IconName::Pin)
            .absolute()
            .top_0()
            .right(px(68.0))
            .disabled(self.sticking)
            .bg(rgba(0x000000))
            .border_0()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| {
                this.stick(window, cx);
            }))
            .into_any_element()
    }

    fn refresh_controls(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        Button::new("refresh")
            .icon(IconName::Refresh)
            .absolute()
            .top_0()
            .right_9()
            .disabled(self.refreshing)
            .bg(rgba(0x000000))
            .border_0()
            .cursor_pointer()
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

    fn stick(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.is_persisted() || self.sticking {
            return;
        }
        self.sticking = true;
        self.error = None;
        cx.notify();

        let title = self.stick_title();
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
                        this.sticking = false;
                        cx.notify();
                    });
                }
                Err(err) => {
                    let _ = entity.update(cx, |this, cx| {
                        this.sticking = false;
                        this.error = Some(format!("Failed to save file sticker: {err:#}"));
                        cx.notify();
                    });
                }
            },
        )
        .detach();
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
}

impl Render for FileSticker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(14.0));
        self.ensure_watch_loop(window, cx);
        self.ensure_audio_anim_loop(cx);

        let bg_color = Rgba {
            a: 0.85,
            ..self.color.bg()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(bg_color)
            .relative()
            .window_control_area(WindowControlArea::Drag)
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
                        .child(Alert::error("file-preview-error", err.as_str())),
                )
            })
            .when(
                (self.preview.is_none() || self.summaries.is_empty()) && self.refreshing,
                |view| view.child(div().p_2().text_sm().opacity(0.85).child("Loading ...")),
            )
            .when(
                self.preview.is_some()
                    && !matches!(
                        self.preview,
                        Some(FilePreview::Audio {
                            state: audio::AudioState {
                                handle: Some(_),
                                ..
                            },
                            ..
                        })
                    )
                    && window.is_window_hovered(),
                |view| {
                    view.child(
                        div()
                            .bg(Rgba {
                                a: 0.95,
                                ..self.color.bg()
                            })
                            .shadow_md()
                            .window_control_area(WindowControlArea::Drag)
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
                view.child(self.stick_controls(cx))
            })
            .when(window.is_window_hovered(), |view| {
                view.child(self.refresh_controls(cx))
            })
    }
}
