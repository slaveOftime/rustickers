use gpui::ImageFormat;
use notify::{EventKind, RecursiveMode};
use std::path::{Path, PathBuf};
use url::Url;

const MAX_TEXT_PREVIEW_BYTES: usize = 1024 * 1024 * 5; // 5 MB

pub fn format_size(bytes: u64) -> String {
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

pub fn file_name_for_display(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

pub fn source_name_for_display(source: &str) -> String {
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

pub fn wrap_code_as_markdown(language: &str, content: &str) -> String {
    let prefix = format!("```{language}\n");
    let mut result = String::with_capacity(prefix.len() + content.len() + 4);
    result.push_str(&prefix);
    result.push_str(content);
    result.push('\n');
    result.push_str("```");
    result
}

pub fn image_format_for_ext(ext: &str) -> Option<ImageFormat> {
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

pub fn read_text_preview(path: &Path) -> (std::io::Result<String>, bool) {
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

pub fn is_binary_text_content(content: &str) -> bool {
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

pub fn is_relevant_watch_event(kind: &EventKind) -> bool {
    !matches!(kind, EventKind::Access(_))
}

pub fn watch_target(raw_path: &str) -> Option<(PathBuf, RecursiveMode)> {
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
