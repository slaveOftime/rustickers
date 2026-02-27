use futures::StreamExt;
use futures::channel::mpsc as async_mpsc;
use gpui::{
    Context, Entity, ObjectFit, Rgba, Window, WindowControlArea, div, img, prelude::*, px,
    relative, rgba,
};
use gpui_component::{
    Disableable, alert::Alert, button::Button, h_flex, scroll::ScrollableElement, text::TextView,
    v_flex,
};
use gpui_component::{Icon, green_500};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
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

const MAX_TEXT_PREVIEW_BYTES: usize = 128 * 1024;
const FILE_WATCH_DEBOUNCE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStickerContent {
    pub files: Vec<String>,
}

impl FileStickerContent {
    pub fn from_sources(sources: &[String]) -> Self {
        Self {
            files: sources.iter().map(|source| source.to_string()).collect(),
        }
    }

    pub fn from_json_or_raw(content: &str) -> Self {
        let parsed = serde_json::from_str::<Self>(content).ok();
        match parsed {
            Some(data) if !data.files.is_empty() => data,
            _ => {
                if content.trim().is_empty() {
                    Self { files: Vec::new() }
                } else {
                    Self {
                        files: vec![content.to_string()],
                    }
                }
            }
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{\"files\":[]}".to_string())
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

struct DirectoryStats {
    size_bytes: u64,
    file_count: u64,
    folder_count: u64,
}

enum FilePreview {
    Markdown(String),
    Text(String),
    Image(PathBuf),
    WebView(Entity<SimpleWebView>),
}

impl FileSticker {
    pub fn new(
        id: i64,
        color: StickerColor,
        store: ArcStickerStore,
        content: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    ) -> Self {
        let parsed = FileStickerContent::from_json_or_raw(content);
        let source_paths = parsed.files;
        let mut summaries: Vec<FileSummary> = source_paths
            .iter()
            .map(|raw_path| FileSummary {
                name: source_name_for_display(raw_path),
                path: raw_path.clone(),
                is_dir: false,
                size_bytes: None,
                file_count: None,
                folder_count: None,
                modified_at_ms: None,
            })
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
        this.spawn_refresh(window, cx);
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
            .icon(IconName::Check)
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
                this.spawn_refresh(window, cx);
            }))
            .into_any_element()
    }

    fn spawn_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                let _ = entity.update_in(cx, move |this, window, cx| {
                    if summaries.len() == 1 {
                        let source = summaries[0].path.as_str();
                        match build_preview(source, window, cx) {
                            Ok(preview) => {
                                this.preview = preview;
                            }
                            Err(err) => {
                                this.error = Some(err);
                            }
                        }
                    }

                    this.summaries = summaries;
                    this.refreshing = false;
                    cx.notify();
                });
            })
            .detach();
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
                                if !this.watch_pending.load(Ordering::Acquire) {
                                    return true;
                                }

                                if this.refreshing {
                                    return false;
                                }

                                this.watch_pending.store(false, Ordering::Release);
                                this.spawn_refresh(window, cx);
                                true
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
                    .unwrap_or_else(|| "Unknown".to_string());
                let modified_text = item
                    .modified_at_ms
                    .map(crate::utils::time::format_unix_millis)
                    .unwrap_or_else(|| "Unknown".to_string());
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
                        _ => Some("Items: Unknown".to_string()),
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
                                    .when_some(size_share_text, |view, text| {
                                        view.child(format!("{text} of total"))
                                    })
                                    .when_some(items_text, |view, text| view.child(text))
                                    .child(format!("Modified: {modified_text}")),
                            ),
                    )
            }))
            .into_any_element()
    }

    fn preview_view(&self) -> gpui::AnyElement {
        match &self.preview {
            Some(FilePreview::Markdown(markdown)) => TextView::markdown("file-markdown", markdown)
                .size_full()
                .selectable(false)
                .scrollable(true)
                .into_any_element(),
            Some(FilePreview::Text(text)) => div()
                .p_2()
                .size_full()
                .overflow_scrollbar()
                .text_sm()
                .child(text.clone())
                .into_any_element(),
            Some(FilePreview::Image(path)) => div()
                .size_full()
                .child(img(path.as_path()).size_full().object_fit(ObjectFit::Cover))
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
                div()
                    .h_full()
                    .flex_shrink()
                    .overflow_hidden()
                    .child(v_flex().overflow_y_scrollbar().child(self.preview_view())),
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
            summaries.push(FileSummary {
                name: source_name_for_display(raw_path.as_str()),
                path: raw_path,
                is_dir: false,
                size_bytes: None,
                file_count: None,
                folder_count: None,
                modified_at_ms: None,
            });
            continue;
        }

        let path = PathBuf::from(raw_path.clone());
        if path.exists() {
            summaries.push(build_summary(path));
        } else {
            summaries.push(FileSummary {
                name: source_name_for_display(raw_path.as_str()),
                path: raw_path,
                is_dir: false,
                size_bytes: None,
                file_count: None,
                folder_count: None,
                modified_at_ms: None,
            });
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
    window: &mut Window,
    cx: &mut Context<FileSticker>,
) -> Result<Option<FilePreview>, String> {
    if crate::utils::url::is_url(source) {
        return Ok(Some(FilePreview::WebView(
            cx.new(|cx| SimpleWebView::new(source, window, cx)),
        )));
    }

    let path = Path::new(source);
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return Ok(None);
    };

    let ext = ext.to_ascii_lowercase();

    if crate::utils::file::is_image_ext(ext.as_str()) {
        return Ok(Some(FilePreview::Image(path.to_path_buf())));
    }

    if crate::utils::file::is_web_doc_ext(ext.as_str()) {
        return crate::utils::url::create_local_file_url(path)
            .map(|url| FilePreview::WebView(cx.new(|cx| SimpleWebView::new(&url, window, cx))))
            .map(Some);
    }

    if crate::utils::file::is_markdown_ext(ext.as_str()) {
        return crate::utils::file::read_text_truncate(path, MAX_TEXT_PREVIEW_BYTES)
            .map(FilePreview::Markdown)
            .map(Some)
            .map_err(|err| format!("Failed to read markdown preview: {err}"));
    }

    if crate::utils::file::is_code_ext(ext.as_str()) {
        let language = crate::utils::file::markdown_language_for_ext(ext.as_str());
        return crate::utils::file::read_text_full(path)
            .map(|content| FilePreview::Markdown(wrap_code_as_markdown(language, content)))
            .map(Some)
            .map_err(|err| format!("Failed to read code preview: {err}"));
    }

    if crate::utils::file::is_text_ext(ext.as_str()) {
        return crate::utils::file::read_text_truncate(path, MAX_TEXT_PREVIEW_BYTES)
            .map(FilePreview::Text)
            .map(Some)
            .map_err(|err| format!("Failed to read text preview: {err}"));
    }

    Ok(None)
}

fn wrap_code_as_markdown(language: &str, mut content: String) -> String {
    let prefix = format!("```{language}\n");
    content.reserve(prefix.len() + 4);
    content.insert_str(0, &prefix);
    content.push('\n');
    content.push_str("```");
    content
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
