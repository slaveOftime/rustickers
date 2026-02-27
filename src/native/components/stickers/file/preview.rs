use gpui::{Context, Entity, Image, Rgba, Window, prelude::*};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use url::Url;

use crate::native::components::stickers::Sticker;
use crate::native::components::webview::SimpleWebView;

pub(super) enum FilePreview {
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
    Audio {
        source_path: PathBuf,
    },
}

impl FilePreview {
    pub(super) fn editable_source(&self) -> Option<&Path> {
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

    pub(super) fn editable_content(&self) -> Option<&str> {
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

    pub(super) fn code_language(&self) -> Option<&str> {
        match self {
            Self::Code { language, .. } => Some(language.as_str()),
            _ => None,
        }
    }

    pub(super) fn replace_content(&mut self, next_content: String) {
        match self {
            Self::Markdown { content, .. }
            | Self::Text { content, .. }
            | Self::Code { content, .. } => *content = next_content,
            _ => {}
        }
    }
}

pub(super) fn build_preview(
    source: &str,
    color: Rgba,
    window: &mut Window,
    cx: &mut Context<super::FileSticker>,
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

    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let ext = ext.as_str();

    if crate::utils::file::is_image_ext(ext) {
        let Some(format) = super::utils::image_format_for_ext(ext) else {
            return Ok(None);
        };
        return match std::fs::read(path) {
            Ok(bytes) => Ok(Some(FilePreview::Image(Arc::new(gpui::Image::from_bytes(
                format, bytes,
            ))))),
            Err(err) => Err(format!("Failed to load image preview: {err}")),
        };
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

    if crate::utils::file::is_audio_ext(ext) {
        return Ok(Some(FilePreview::Audio {
            source_path: path.to_path_buf(),
        }));
    }

    if crate::utils::file::is_markdown_ext(ext) {
        let (content_result, editable) = super::utils::read_text_preview(path);
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

    let (content_result, editable) = super::utils::read_text_preview(path);
    content_result
        .map(|content| {
            if super::utils::is_binary_text_content(content.as_str()) {
                None
            } else {
                Some(FilePreview::Text {
                    source_path: path.to_path_buf(),
                    content,
                    editable,
                })
            }
        })
        .map_err(|err| format!("Failed to read text preview: {err}"))
}

pub(super) fn build_default_size(sources: &[&str]) -> gpui::Size<i32> {
    if sources.is_empty() {
        return <super::FileSticker as Sticker>::default_window_size();
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
            return gpui::size(360, 160);
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
