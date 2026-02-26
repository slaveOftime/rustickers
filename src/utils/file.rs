use std::path::Path;

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
    matches!(ext, "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg")
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
