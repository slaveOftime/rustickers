use std::path::Path;

use gpui::ImageFormat;

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

pub fn read_text_full(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

pub fn read_text_truncate(path: &Path, max_bytes: usize) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let bytes = if bytes.len() > max_bytes {
        &bytes[..max_bytes]
    } else {
        bytes.as_slice()
    };

    Ok(String::from_utf8_lossy(bytes).to_string())
}

pub fn is_image_ext(ext: &str) -> bool {
    matches!(
        ext,
        "avif"
            | "jpg"
            | "jpeg"
            | "png"
            | "gif"
            | "webp"
            | "tif"
            | "tiff"
            | "tga"
            | "dds"
            | "bmp"
            | "ico"
            | "hdr"
            | "exr"
            | "pbm"
            | "pam"
            | "ppm"
            | "pgm"
            | "ff"
            | "farbfeld"
            | "qoi"
            | "svg"
    )
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

pub fn is_markdown_ext(ext: &str) -> bool {
    matches!(ext, "md" | "markdown")
}

pub fn is_web_doc_ext(ext: &str) -> bool {
    matches!(ext, "html" | "htm" | "pdf")
}

pub fn is_code_ext(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "py"
            | "js"
            | "ts"
            | "jsx"
            | "tsx"
            | "java"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "cs"
            | "go"
            | "rb"
            | "php"
            | "swift"
            | "kt"
            | "kts"
            | "scala"
            | "r"
            | "m"
            | "sh"
            | "bash"
            | "zsh"
            | "ps1"
            | "lua"
            | "dart"
            | "ex"
            | "exs"
            | "erl"
            | "hrl"
            | "clj"
            | "cljs"
            | "fs"
            | "fsx"
            | "vb"
            | "sql"
            | "html"
            | "css"
            | "scss"
            | "less"
            | "xml"
            | "yaml"
            | "yml"
            | "toml"
            | "json"
            | "pl"
            | "pm"
            | "groovy"
            | "gradle"
    )
}

pub fn is_text_ext(ext: &str) -> bool {
    matches!(
        ext,
        "txt" | "log" | "json" | "toml" | "yaml" | "yml" | "rs" | "js" | "ts" | "css" | "csv"
    )
}

pub fn is_video_ext(ext: &str) -> bool {
    matches!(
        ext,
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "mpeg" | "mpg" | "3gp"
    )
}

pub fn is_audio_ext(ext: &str) -> bool {
    matches!(
        ext,
        "mp3" | "wav" | "flac" | "ogg" | "aac" | "m4a" | "wma" | "opus" | "aiff" | "aif"
    )
}

pub fn markdown_language_for_ext(ext: &str) -> &'static str {
    match ext {
        "rs" => "rust",
        "py" => "python",
        "js" => "javascript",
        "ts" => "typescript",
        "jsx" => "jsx",
        "tsx" => "tsx",
        "java" => "java",
        "c" => "c",
        "h" => "c",
        "cpp" => "cpp",
        "hpp" => "cpp",
        "cs" => "csharp",
        "go" => "go",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "scala" => "scala",
        "r" => "r",
        "m" => "matlab",
        "sh" | "bash" | "zsh" => "bash",
        "ps1" => "powershell",
        "lua" => "lua",
        "dart" => "dart",
        "ex" | "exs" => "elixir",
        "erl" | "hrl" => "erlang",
        "clj" | "cljs" => "clojure",
        "fs" | "fsx" => "fsharp",
        "vb" => "vbnet",
        "sql" => "sql",
        "html" => "html",
        "css" => "css",
        "scss" => "scss",
        "less" => "less",
        "xml" => "xml",
        "json" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "pl" | "pm" => "perl",
        "groovy" | "gradle" => "groovy",
        _ => "text",
    }
}
