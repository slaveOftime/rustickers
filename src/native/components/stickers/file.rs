use futures::StreamExt;
use futures::channel::mpsc as async_mpsc;
use gpui::{
    Context, Entity, Image, ImageFormat, ImageSource, KeyDownEvent, MouseButton, MouseDownEvent,
    ObjectFit, Rgba, Window, WindowControlArea, div, img, prelude::*, px, relative, rgba,
};
use gpui_component::Sizable;
use gpui_component::{
    Disableable,
    alert::Alert,
    button::Button,
    h_flex,
    input::{Input, InputState},
    scroll::ScrollableElement,
    text::TextView,
    v_flex,
};
use gpui_component::{Icon, green_500};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;
use std::time::UNIX_EPOCH;
use url::Url;

use crate::model::sticker::{StickerColor, StickerDetail, StickerState, StickerType};
use crate::native::components::IconName;
use crate::native::components::webview::SimpleWebView;
use crate::native::windows::StickerWindowEvent;
use crate::native::windows::sticker::StickerWindow;
use crate::storage::ArcStickerStore;

const MAX_TEXT_PREVIEW_BYTES: usize = 1024 * 1024 * 5; // 5 MB
const FILE_WATCH_DEBOUNCE: Duration = Duration::from_millis(100);
const EMPTY_FILE_LIST_JSON: &str = "{\"files\":[]}";
const EDIT_HINT_TEXT: &str = "double-click to edit";
const UNKNOWN_TEXT: &str = "Unknown";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStickerContent {
    pub files: Vec<String>,
}

impl FileStickerContent {
    pub fn from_sources(sources: &[String]) -> Self {
        Self {
            files: sources.iter().map(|source| source.to_owned()).collect(),
        }
    }

    pub fn from_json_or_raw(content: &str) -> Self {
        if let Ok(data) = serde_json::from_str::<Self>(content)
            && !data.files.is_empty()
        {
            return data;
        }

        if content.trim().is_empty() {
            Self { files: Vec::new() }
        } else {
            Self {
                files: vec![content.to_owned()],
            }
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| EMPTY_FILE_LIST_JSON.to_owned())
    }
}

pub struct FileSticker {
    id: i64,
    color: StickerColor,
    store: ArcStickerStore,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    source_paths: Vec<String>,
    summaries: Vec<FileSummary>,
    preview: Option<FilePreview>,
    preview_editor: Option<Entity<InputState>>,
    refreshing: bool,
    sticking: bool,
    error: Option<String>,
    watcher: Option<RecommendedWatcher>,
    watch_events_rx: Option<async_mpsc::UnboundedReceiver<()>>,
    watch_loop_started: bool,
    watch_pending: Arc<AtomicBool>,
    watch_stop: Arc<AtomicBool>,
}

struct FileSummary {
    name: String,
    path: String,
    is_dir: bool,
    size_bytes: Option<u64>,
    file_count: Option<u64>,
    folder_count: Option<u64>,
    modified_at_ms: Option<i64>,
}

impl FileSummary {
    fn from_source(source: &str) -> Self {
        Self {
            name: source_name_for_display(source),
            path: source.to_string(),
            is_dir: false,
            size_bytes: None,
            file_count: None,
            folder_count: None,
            modified_at_ms: None,
        }
    }
}

struct DirectoryStats {
    size_bytes: u64,
    file_count: u64,
    folder_count: u64,
}

enum FilePreview {
    Markdown {
        source_path: PathBuf,
        content: String,
        editable: bool,
    },
    Text {
        source_path: PathBuf,
        content: String,
        editable: bool,
    },
    Code {
        source_path: PathBuf,
        content: String,
        language: String,
        editable: bool,
    },
    Image(Arc<Image>),
    WebView(Entity<SimpleWebView>),
}

impl FilePreview {
    fn editable_source(&self) -> Option<&Path> {
        match self {
            Self::Markdown {
                source_path,
                editable: true,
                ..
            }
            | Self::Text {
                source_path,
                editable: true,
                ..
            }
            | Self::Code {
                source_path,
                editable: true,
                ..
            } => Some(source_path.as_path()),
            _ => None,
        }
    }

    fn editable_content(&self) -> Option<&str> {
        match self {
            Self::Markdown {
                content,
                editable: true,
                ..
            }
            | Self::Text {
                content,
                editable: true,
                ..
            }
            | Self::Code {
                content,
                editable: true,
                ..
            } => Some(content.as_str()),
            _ => None,
        }
    }

    fn code_language(&self) -> Option<&str> {
        match self {
            Self::Code { language, .. } => Some(language.as_str()),
            _ => None,
        }
    }

    fn replace_content(&mut self, next_content: String) {
        match self {
            Self::Markdown { content, .. }
            | Self::Text { content, .. }
            | Self::Code { content, .. } => {
                *content = next_content;
            }
            _ => {}
        }
    }
}

impl FileSticker {
    pub fn default_window_size_for_sources(sources: &[&str]) -> gpui::Size<i32> {
        build_default_size(sources)
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

    fn handle_preview_double_click(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.click_count >= 2 {
            self.start_preview_edit(window, cx);
        }
    }

    fn maybe_edit_hint(&self, editable: bool) -> Option<gpui::AnyElement> {
        editable.then(|| {
            div()
                .absolute()
                .right_2()
                .top_8()
                .text_xs()
                .opacity(0.7)
                .child(EDIT_HINT_TEXT)
                .into_any_element()
        })
    }

    fn spawn_refresh_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = (self.source_paths.len() == 1).then(|| self.source_paths[0].to_owned());

        let Some(source) = source else {
            self.preview = None;
            self.preview_editor = None;
            cx.notify();
            return;
        };

        let entity = cx.entity();
        window
            .spawn(cx, async move |cx| {
                let _ = entity.update_in(cx, move |this, window, cx| {
                    let bg = Rgba {
                        a: 0.5,
                        ..this.color.bg()
                    };
                    match build_preview(source.as_str(), bg, window, cx) {
                        Ok(preview) => {
                            this.preview = preview;
                            this.preview_editor = None;
                        }
                        Err(err) => {
                            this.preview = None;
                            this.preview_editor = None;
                            this.error = Some(err);
                        }
                    }

                    cx.notify();
                });
            })
            .detach();
    }

    fn spawn_refresh_summaries(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }

        self.refreshing = true;
        self.error = None;
        cx.notify();

        let entity = cx.entity();
        let source_paths = self.source_paths.clone();
        window
            .spawn(cx, async move |cx| {
                let summaries = build_summaries(source_paths);
                let _ = entity.update_in(cx, move |this, _window, cx| {
                    this.summaries = summaries;
                    this.refreshing = false;
                    cx.notify();
                });
            })
            .detach();
    }

    fn start_preview_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let initial_content = match self
            .preview
            .as_ref()
            .and_then(FilePreview::editable_content)
        {
            Some(content) => content.to_string(),
            None => return,
        };
        let code_language = self
            .preview
            .as_ref()
            .and_then(FilePreview::code_language)
            .map(|language| language.to_string());

        self.preview_editor = Some(cx.new(move |cx| {
            let mut state = InputState::new(window, cx)
                .multi_line(true)
                .searchable(true)
                .placeholder("Edit file content, ctrl+s to save")
                .default_value(initial_content);
            if let Some(language) = code_language.as_ref() {
                state = state.code_editor(language);
            }
            state
        }));
        self.error = None;
        cx.notify();
    }

    fn save_preview_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.preview_editor.as_ref() else {
            return;
        };

        let content = editor.read(cx).value().to_string();
        let save_path = match self
            .preview
            .as_ref()
            .and_then(FilePreview::editable_source)
            .map(Path::to_path_buf)
        {
            Some(path) => path,
            None => return,
        };

        match std::fs::write(&save_path, content.as_bytes()) {
            Ok(_) => {
                if let Some(preview) = self.preview.as_mut() {
                    preview.replace_content(content);
                }

                self.preview_editor = None;
                self.error = None;
                self.spawn_refresh_summaries(window, cx);
            }
            Err(err) => {
                self.error = Some(format!("Failed to save preview file: {err}"));
            }
        }

        cx.notify();
    }

    fn init_file_watcher(&mut self) {
        let (event_tx, event_rx) = async_mpsc::unbounded::<()>();
        let watch_pending = Arc::clone(&self.watch_pending);
        let watch_stop = Arc::clone(&self.watch_stop);

        let mut watcher = match RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| {
                if watch_stop.load(Ordering::Acquire) {
                    return;
                }

                let Ok(event) = result else {
                    return;
                };

                if !is_relevant_watch_event(&event.kind) {
                    return;
                }

                if !watch_pending.swap(true, Ordering::AcqRel) {
                    let _ = event_tx.unbounded_send(());
                }
            },
            Config::default(),
        ) {
            Ok(watcher) => watcher,
            Err(err) => {
                self.error = Some(format!("Failed to initialize file watcher: {err}"));
                return;
            }
        };

        for raw_path in &self.source_paths {
            let Some((watch_path, mode)) = watch_target(raw_path) else {
                continue;
            };
            if let Err(err) = watcher.watch(&watch_path, mode) {
                tracing::warn!(
                    path = %watch_path.to_string_lossy(),
                    error = %err,
                    "Failed to watch sticker source path"
                );
            }
        }

        self.watcher = Some(watcher);
        self.watch_events_rx = Some(event_rx);
    }

    fn ensure_watch_loop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.watch_loop_started {
            return;
        }

        let Some(mut event_rx) = self.watch_events_rx.take() else {
            return;
        };

        self.watch_loop_started = true;
        let entity = cx.entity();
        let watch_stop = Arc::clone(&self.watch_stop);

        window
            .spawn(cx, async move |cx| {
                while event_rx.next().await.is_some() {
                    if watch_stop.load(Ordering::Acquire) {
                        break;
                    }

                    loop {
                        cx.background_executor().timer(FILE_WATCH_DEBOUNCE).await;

                        if watch_stop.load(Ordering::Acquire) {
                            break;
                        }

                        let should_break = entity
                            .update_in(cx, |this, window, cx| {
                                this.refresh_from_watch_if_ready(window, cx)
                            })
                            .unwrap_or(true);

                        if should_break {
                            break;
                        }
                    }
                }
            })
            .detach();
    }

    fn refresh_from_watch_if_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.watch_pending.load(Ordering::Acquire) {
            return true;
        }

        if self.refreshing {
            return false;
        }

        self.watch_pending.store(false, Ordering::Release);
        self.spawn_refresh_preview(window, cx);
        self.spawn_refresh_summaries(window, cx);
        true
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

    fn summary_view(&self) -> gpui::AnyElement {
        let show_size_share = self.summaries.len() > 1;
        let total_known_size = if show_size_share {
            self.summaries
                .iter()
                .filter_map(|item| item.size_bytes)
                .sum::<u64>()
        } else {
            0
        };

        v_flex()
            .window_control_area(WindowControlArea::Drag)
            .when(self.refreshing, |view| {
                view.child(
                    div()
                        .p_2()
                        .text_xs()
                        .opacity(0.85)
                        .child("Refreshing summaries..."),
                )
            })
            .children(self.summaries.iter().map(|item| {
                let size_text = item
                    .size_bytes
                    .map(format_size)
                    .unwrap_or_else(|| UNKNOWN_TEXT.to_owned());
                let modified_text = item
                    .modified_at_ms
                    .map(crate::utils::time::format_unix_millis)
                    .unwrap_or_else(|| UNKNOWN_TEXT.to_owned());
                let size_share_percent = if show_size_share && total_known_size > 0 {
                    item.size_bytes
                        .map(|size| size as f32 / total_known_size as f32)
                } else {
                    None
                };
                let size_share_text =
                    size_share_percent.map(|percent| format!("{:.0}%", percent * 100.0));
                let items_text = if item.is_dir {
                    match (item.file_count, item.folder_count) {
                        (Some(file_count), Some(folder_count)) => {
                            Some(format!("Items: {file_count} files, {folder_count} folders"))
                        }
                        _ => Some(format!("Items: {UNKNOWN_TEXT}")),
                    }
                } else {
                    None
                };

                div()
                    .p_2()
                    .rounded_md()
                    .relative()
                    .when_some(size_share_percent, |view, p| {
                        view.child(
                            div()
                                .bg(green_500().alpha(0.2))
                                .w(relative(p))
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0(),
                        )
                    })
                    .child(
                        v_flex()
                            .gap_1()
                            .text_sm()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .when(item.is_dir, |view| {
                                        view.child(Icon::new(IconName::Folder))
                                    })
                                    .child(item.name.clone()),
                            )
                            .child(div().text_xs().opacity(0.9).child(item.path.clone()))
                            .child(
                                h_flex()
                                    .text_xs()
                                    .opacity(0.8)
                                    .gap_3()
                                    .flex_wrap()
                                    .child(format!("Size: {size_text}"))
                                    .when_some(size_share_text, |view, text| view.child(text))
                                    .when_some(items_text, |view, text| view.child(text))
                                    .child(format!("Modified: {modified_text}")),
                            ),
                    )
            }))
            .into_any_element()
    }

    fn preview_view(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        if let Some(editor) = self.preview_editor.as_ref() {
            return v_flex()
                .size_full()
                .gap_1()
                .p_1()
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.modifiers.control
                        && event.keystroke.key.eq_ignore_ascii_case("s")
                    {
                        this.save_preview_edit(window, cx);
                    }
                }))
                .child(
                    Input::new(editor)
                        .size_full()
                        .bordered(false)
                        .bg(rgba(0x000000)),
                )
                .child(
                    h_flex().child(
                        Button::new("save-preview-file")
                            .label("Save (ctrl+s)")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_preview_edit(window, cx);
                            })),
                    ),
                )
                .into_any_element();
        }

        match &self.preview {
            Some(FilePreview::Markdown {
                content, editable, ..
            }) => div()
                .p_2()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.handle_preview_double_click(event, window, cx);
                    }),
                )
                .child(
                    TextView::markdown("file-markdown", content)
                        .size_full()
                        .selectable(false)
                        .scrollable(true),
                )
                .when_some(
                    self.maybe_edit_hint(*editable && window.is_window_hovered()),
                    |view, hint| view.child(hint),
                )
                .into_any_element(),
            Some(FilePreview::Text {
                content, editable, ..
            }) => div()
                .p_2()
                .size_full()
                .overflow_scrollbar()
                .text_sm()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.handle_preview_double_click(event, window, cx);
                    }),
                )
                .child(content.clone())
                .when_some(
                    self.maybe_edit_hint(*editable && window.is_window_hovered()),
                    |view, hint| view.child(hint),
                )
                .into_any_element(),
            Some(FilePreview::Code {
                content,
                editable,
                language,
                ..
            }) => div()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.handle_preview_double_click(event, window, cx);
                    }),
                )
                .child(
                    TextView::markdown("file-code", wrap_code_as_markdown(language, content))
                        .size_full()
                        .selectable(false)
                        .scrollable(true),
                )
                .when_some(
                    self.maybe_edit_hint(*editable && window.is_window_hovered()),
                    |view, hint| view.child(hint),
                )
                .into_any_element(),
            Some(FilePreview::Image(image)) => div()
                .size_full()
                .child(
                    img(ImageSource::Image(image.clone()))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element(),
            Some(FilePreview::WebView(webview)) => {
                div().size_full().child(webview.clone()).into_any_element()
            }
            None => div().child(self.summary_view()).into_any_element(),
        }
    }
}

impl super::Sticker for FileSticker {
    fn id(&self) -> i64 {
        self.id
    }

    fn save_on_close(&mut self, _cx: &mut Context<Self>) -> bool {
        self.watch_stop.store(true, Ordering::Release);
        self.watcher = None;
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
                self.preview.is_some() && window.is_window_hovered(),
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

fn is_relevant_watch_event(kind: &EventKind) -> bool {
    !matches!(kind, EventKind::Access(_))
}

fn watch_target(raw_path: &str) -> Option<(PathBuf, RecursiveMode)> {
    if crate::utils::url::is_url(raw_path) {
        return None;
    }

    let path = PathBuf::from(raw_path);
    if path.is_dir() {
        return Some((path, RecursiveMode::Recursive));
    }

    if path.is_file() {
        return Some((path, RecursiveMode::NonRecursive));
    }

    path.parent()
        .filter(|parent| parent.exists())
        .map(|parent| (parent.to_path_buf(), RecursiveMode::NonRecursive))
}

fn build_summaries(source_paths: Vec<String>) -> Vec<FileSummary> {
    let mut summaries = Vec::with_capacity(source_paths.len());
    for raw_path in source_paths {
        if crate::utils::url::is_url(raw_path.as_str()) {
            summaries.push(FileSummary::from_source(raw_path.as_str()));
            continue;
        }

        let path = PathBuf::from(raw_path.as_str());
        if path.exists() {
            summaries.push(build_summary(path));
        } else {
            summaries.push(FileSummary::from_source(raw_path.as_str()));
        }
    }

    summaries.sort_by(|a, b| {
        b.size_bytes
            .unwrap_or(0)
            .cmp(&a.size_bytes.unwrap_or(0))
            .then_with(|| a.name.cmp(&b.name))
    });

    summaries
}

fn build_summary(path: PathBuf) -> FileSummary {
    let metadata = std::fs::metadata(&path).ok();
    let is_dir = metadata.as_ref().is_some_and(|m| m.is_dir());
    let dir_stats = if is_dir {
        summarize_directory(path.as_path())
    } else {
        None
    };
    let size_bytes = dir_stats
        .as_ref()
        .map(|stats| stats.size_bytes)
        .or_else(|| metadata.as_ref().map(|m| m.len()));
    let modified_at_ms = metadata
        .and_then(|m| m.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64);

    FileSummary {
        name: file_name_for_display(path.as_path()),
        path: path.to_string_lossy().to_string(),
        is_dir,
        size_bytes,
        file_count: dir_stats.as_ref().map(|stats| stats.file_count),
        folder_count: dir_stats.as_ref().map(|stats| stats.folder_count),
        modified_at_ms,
    }
}

fn summarize_directory(root: &Path) -> Option<DirectoryStats> {
    use ignore::{WalkBuilder, WalkState};

    fn atomic_saturating_add(counter: &AtomicU64, value: u64) {
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_add(value))
        });
    }

    let root_path = Arc::new(root.to_path_buf());
    let size_bytes = Arc::new(AtomicU64::new(0));
    let file_count = Arc::new(AtomicU64::new(0));
    let folder_count = Arc::new(AtomicU64::new(0));

    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .require_git(false);

    builder.build_parallel().run(|| {
        let root_path = Arc::clone(&root_path);
        let size_bytes = Arc::clone(&size_bytes);
        let file_count = Arc::clone(&file_count);
        let folder_count = Arc::clone(&folder_count);

        Box::new(move |result| {
            let entry = match result {
                Ok(entry) => entry,
                Err(_) => return WalkState::Continue,
            };

            if entry.path() == root_path.as_path() {
                return WalkState::Continue;
            }

            let file_type = match entry.file_type() {
                Some(file_type) => file_type,
                None => return WalkState::Continue,
            };

            if file_type.is_dir() {
                atomic_saturating_add(&folder_count, 1);
            } else if file_type.is_file() {
                atomic_saturating_add(&file_count, 1);
                if let Ok(metadata) = entry.metadata() {
                    atomic_saturating_add(&size_bytes, metadata.len());
                }
            }

            WalkState::Continue
        })
    });

    Some(DirectoryStats {
        size_bytes: size_bytes.load(Ordering::Relaxed),
        file_count: file_count.load(Ordering::Relaxed),
        folder_count: folder_count.load(Ordering::Relaxed),
    })
}

fn build_preview(
    source: &str,
    color: Rgba,
    window: &mut Window,
    cx: &mut Context<FileSticker>,
) -> Result<Option<FilePreview>, String> {
    if crate::utils::url::is_url(source) {
        return Ok(Some(FilePreview::WebView(cx.new(|cx| {
            let mut view = SimpleWebView::new(source, window, cx);
            view.set_bg(color, cx);
            view
        }))));
    }

    let path = Path::new(source);
    if path.is_dir() {
        return Ok(None);
    }

    let ext = &path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .to_string();

    if crate::utils::file::is_image_ext(ext) {
        let Some(format) = image_format_for_ext(ext) else {
            return Ok(None);
        };
        match fs::read(path) {
            Ok(bytes) => {
                return Ok(Some(FilePreview::Image(Arc::new(Image::from_bytes(
                    format, bytes,
                )))));
            }
            Err(err) => return Err(format!("Failed to load image preview: {err}")),
        }
    }

    if crate::utils::file::is_web_doc_ext(ext) || crate::utils::file::is_video_ext(ext) {
        return crate::utils::url::create_local_file_url(path)
            .map(|url| {
                FilePreview::WebView(cx.new(|cx| {
                    let mut view = SimpleWebView::new(&url, window, cx);
                    view.set_bg(color, cx);
                    view
                }))
            })
            .map(Some);
    }

    if crate::utils::file::is_markdown_ext(ext) {
        let (content_result, editable) = read_text_preview(path);
        return content_result
            .map(|content| FilePreview::Markdown {
                source_path: path.to_path_buf(),
                content,
                editable,
            })
            .map(Some)
            .map_err(|err| format!("Failed to read markdown preview: {err}"));
    }

    if crate::utils::file::is_code_ext(ext) {
        let language = crate::utils::file::markdown_language_for_ext(ext);
        return crate::utils::file::read_text_full(path)
            .map(|content| FilePreview::Code {
                source_path: path.to_path_buf(),
                content,
                language: language.to_string(),
                editable: true,
            })
            .map(Some)
            .map_err(|err| format!("Failed to read code preview: {err}"));
    }

    let (content_result, editable) = read_text_preview(path);
    return content_result
        .map(|content| {
            if is_binary_text_content(content.as_str()) {
                None
            } else {
                Some(FilePreview::Text {
                    source_path: path.to_path_buf(),
                    content,
                    editable,
                })
            }
        })
        .map_err(|err| format!("Failed to read text preview: {err}"));
}

fn build_default_size(sources: &[&str]) -> gpui::Size<i32> {
    if sources.is_empty() {
        return <FileSticker as super::Sticker>::default_window_size();
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

    return gpui::size(400, 200);
}

fn wrap_code_as_markdown(language: &str, content: &str) -> String {
    let prefix = format!("```{language}\n");
    let mut result = String::with_capacity(prefix.len() + content.len() + 4);
    result.push_str(&prefix);
    result.push_str(content);
    result.push('\n');
    result.push_str("```");
    result
}

fn image_format_for_ext(ext: &str) -> Option<ImageFormat> {
    match ext {
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "png" => Some(ImageFormat::Png),
        "gif" => Some(ImageFormat::Gif),
        "bmp" => Some(ImageFormat::Bmp),
        "svg" => Some(ImageFormat::Svg),
        "webp" => Some(ImageFormat::Webp),
        "ico" => Some(ImageFormat::Ico),
        "tiff" | "tif" => Some(ImageFormat::Tiff),
        _ => None,
    }
}

fn read_text_preview(path: &Path) -> (std::io::Result<String>, bool) {
    let is_small_text = std::fs::metadata(path)
        .map(|metadata| metadata.len() <= MAX_TEXT_PREVIEW_BYTES as u64)
        .unwrap_or(false);

    let content = if is_small_text {
        crate::utils::file::read_text_full(path)
    } else {
        crate::utils::file::read_text_truncate(path, MAX_TEXT_PREVIEW_BYTES)
    };

    (content, is_small_text)
}

fn is_binary_text_content(content: &str) -> bool {
    if content.is_empty() {
        return false;
    }

    if content.as_bytes().contains(&0) {
        return true;
    }

    let sample = content.as_bytes();
    let control_count = sample
        .iter()
        .filter(|&&byte| matches!(byte, 0x01..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0x7F))
        .count();

    control_count > sample.len() / 10
}

fn file_name_for_display(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn source_name_for_display(source: &str) -> String {
    if crate::utils::url::is_url(source)
        && let Ok(url) = Url::parse(source)
    {
        if let Some(last_segment) = url.path_segments().and_then(|segments| segments.last())
            && !last_segment.is_empty()
        {
            return last_segment.to_string();
        }

        if let Some(host) = url.host_str()
            && !host.is_empty()
        {
            return host.to_string();
        }

        return source.to_string();
    }

    file_name_for_display(Path::new(source))
}

fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;

    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}
