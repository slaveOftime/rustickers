use gpui::{
    Context, Entity, ObjectFit, Rgba, Window, WindowControlArea, div, img, prelude::*, px, rgba,
};
use gpui_component::{
    Disableable, alert::Alert, button::Button, h_flex, scroll::ScrollableElement, text::TextView,
    v_flex,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::UNIX_EPOCH;

use crate::model::sticker::{StickerColor, StickerDetail, StickerState, StickerType};
use crate::native::components::IconName;
use crate::native::components::webview::SimpleWebView;
use crate::native::windows::StickerWindowEvent;
use crate::storage::ArcStickerStore;

const MAX_TEXT_PREVIEW_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStickerContent {
    pub files: Vec<String>,
}

impl FileStickerContent {
    pub fn from_paths(paths: &[PathBuf]) -> Self {
        Self {
            files: paths
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect(),
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
    summaries: Vec<FileSummary>,
    preview: Option<FilePreview>,
    sticking: bool,
    error: Option<String>,
}

struct FileSummary {
    name: String,
    path: String,
    size_bytes: Option<u64>,
    modified_at_ms: Option<i64>,
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
        let mut summaries = Vec::new();
        let mut error = None;

        for raw_path in parsed.files {
            let path = PathBuf::from(raw_path.clone());
            if path.exists() {
                summaries.push(build_summary(path));
            } else {
                summaries.push(FileSummary {
                    name: file_name_for_display(Path::new(&raw_path)),
                    path: raw_path,
                    size_bytes: None,
                    modified_at_ms: None,
                });
            }
        }

        let preview = if summaries.len() == 1 {
            let path = PathBuf::from(&summaries[0].path);
            match build_preview(path.as_path(), window, cx) {
                Ok(preview) => preview,
                Err(err) => {
                    error = Some(err);
                    None
                }
            }
        } else {
            None
        };

        Self {
            id,
            color,
            store,
            sticker_events_tx,
            summaries,
            preview,
            sticking: false,
            error,
        }
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
            .right_9()
            .disabled(self.sticking)
            .bg(rgba(0x000000))
            .border_0()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| {
                this.stick(window, cx);
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
        cx.spawn(
            async move |entity, cx| match store.insert_sticker(detail).await {
                Ok(id) => {
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
        v_flex()
            .gap_2()
            .window_control_area(WindowControlArea::Drag)
            .children(self.summaries.iter().map(|item| {
                let size_text = item
                    .size_bytes
                    .map(format_size)
                    .unwrap_or_else(|| "Unknown".to_string());
                let modified_text = item
                    .modified_at_ms
                    .map(crate::utils::time::format_unix_millis)
                    .unwrap_or_else(|| "Unknown".to_string());

                v_flex()
                    .gap_1()
                    .text_sm()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
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
                            .child(format!("Modified: {modified_text}")),
                    )
            }))
            .into_any_element()
    }

    fn preview_view(&self) -> gpui::AnyElement {
        match &self.preview {
            Some(FilePreview::Markdown(markdown)) => TextView::markdown("file-markdown", markdown)
                .p_2()
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
            None => div().p_2().child(self.summary_view()).into_any_element(),
        }
    }
}

impl super::Sticker for FileSticker {
    fn id(&self) -> i64 {
        self.id
    }

    fn save_on_close(&mut self, _cx: &mut Context<Self>) -> bool {
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
                            .p_2()
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
    }
}

fn build_summary(path: PathBuf) -> FileSummary {
    let metadata = std::fs::metadata(&path).ok();
    let size_bytes = metadata.as_ref().map(|m| m.len());
    let modified_at_ms = metadata
        .and_then(|m| m.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64);

    FileSummary {
        name: file_name_for_display(path.as_path()),
        path: path.to_string_lossy().to_string(),
        size_bytes,
        modified_at_ms,
    }
}

fn build_preview(
    path: &Path,
    window: &mut Window,
    cx: &mut Context<FileSticker>,
) -> Result<Option<FilePreview>, String> {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return Ok(None);
    };

    let ext = ext.to_ascii_lowercase();

    if is_image_ext(ext.as_str()) {
        return Ok(Some(FilePreview::Image(path.to_path_buf())));
    }

    if is_markdown_ext(ext.as_str()) {
        return read_text(path)
            .map(FilePreview::Markdown)
            .map(Some)
            .map_err(|err| format!("Failed to read markdown preview: {err}"));
    }

    if is_web_doc_ext(ext.as_str()) {
        return file_url(path)
            .map(|url| FilePreview::WebView(cx.new(|cx| SimpleWebView::new(&url, window, cx))))
            .map(Some);
    }

    if is_text_ext(ext.as_str()) {
        return read_text(path)
            .map(FilePreview::Text)
            .map(Some)
            .map_err(|err| format!("Failed to read text preview: {err}"));
    }

    Ok(None)
}

fn file_url(path: &Path) -> Result<String, String> {
    let canonical_path = path
        .canonicalize()
        .map_err(|err| format!("Failed to resolve file path: {err}"))?;

    Ok(format!(
        "local://localhost/{}",
        canonical_path.to_string_lossy()
    ))
}

fn read_text(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let bytes = if bytes.len() > MAX_TEXT_PREVIEW_BYTES {
        &bytes[..MAX_TEXT_PREVIEW_BYTES]
    } else {
        bytes.as_slice()
    };

    Ok(String::from_utf8_lossy(bytes).to_string())
}

fn file_name_for_display(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn is_image_ext(ext: &str) -> bool {
    matches!(ext, "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg")
}

fn is_markdown_ext(ext: &str) -> bool {
    matches!(ext, "md" | "markdown")
}

fn is_web_doc_ext(ext: &str) -> bool {
    matches!(ext, "html" | "htm" | "pdf")
}

fn is_text_ext(ext: &str) -> bool {
    matches!(
        ext,
        "txt" | "log" | "json" | "toml" | "yaml" | "yml" | "rs" | "js" | "ts" | "css" | "csv"
    )
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
