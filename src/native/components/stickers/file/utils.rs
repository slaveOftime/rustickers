use std::path::Path;
use url::Url;

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
