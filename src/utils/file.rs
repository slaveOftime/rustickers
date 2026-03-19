use std::path::Path;

use gpui::ImageFormat;

pub fn format_terminal_output_for_preview(content: &str) -> String {
    let mut formatted = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => {
                let Some(next) = chars.next() else {
                    formatted.push_str("<ESC>");
                    break;
                };

                match next {
                    '[' => {
                        let mut sequence = String::new();
                        let mut final_byte = None;
                        for ch in chars.by_ref() {
                            sequence.push(ch);
                            if ('@'..='~').contains(&ch) {
                                final_byte = Some(ch);
                                break;
                            }
                        }
                        formatted.push_str(&format_csi_sequence(&sequence, final_byte));
                    }
                    ']' => {
                        let mut sequence = String::new();
                        let mut prev_was_escape = false;
                        for ch in chars.by_ref() {
                            if ch == '\u{7}' {
                                formatted.push_str(&format!("<OSC {} BEL>", sequence));
                                break;
                            }
                            if ch == '\u{7}' || (prev_was_escape && ch == '\\') {
                                if prev_was_escape {
                                    sequence.pop();
                                }
                                formatted.push_str(&format!("<OSC {} ST>", sequence));
                                break;
                            }
                            sequence.push(ch);
                            prev_was_escape = ch == '\u{1b}';
                        }
                    }
                    'P' | 'X' | '^' | '_' => {
                        let control_name = match next {
                            'P' => "DCS",
                            'X' => "SOS",
                            '^' => "PM",
                            '_' => "APC",
                            _ => unreachable!(),
                        };
                        let mut sequence = String::new();
                        let mut prev_was_escape = false;
                        for ch in chars.by_ref() {
                            if prev_was_escape && ch == '\\' {
                                sequence.pop();
                                formatted.push_str(&format!("<{} {} ST>", control_name, sequence));
                                break;
                            }
                            sequence.push(ch);
                            prev_was_escape = ch == '\u{1b}';
                        }
                    }
                    '(' | ')' | '*' | '+' | '-' | '.' | '/' => {
                        if let Some(designator) = chars.next() {
                            formatted.push_str(&format_charset_sequence(next, designator));
                        } else {
                            formatted.push_str(&format!("<ESC {}>", next));
                        }
                    }
                    _ => formatted.push_str(&format_escape_sequence(next)),
                }
            }
            '\r' => {
                if matches!(chars.peek(), Some('\n')) {
                    formatted.push('\n');
                    chars.next();
                } else {
                    formatted.push_str("<CR>");
                }
            }
            '\u{8}' => formatted.push_str("<BS>"),
            '\n' | '\t' => formatted.push(ch),
            ch if ch.is_control() => {
                if let Some(name) = control_name(ch) {
                    formatted.push_str(&format!("<{}>", name));
                }
            }
            _ => formatted.push(ch),
        }
    }

    formatted
}

fn format_csi_sequence(sequence: &str, final_byte: Option<char>) -> String {
    let Some(final_byte) = final_byte else {
        return format!("<ESC[{}>", sequence);
    };

    let is_private = matches!(sequence.chars().next(), Some('<' | '=' | '>' | '?'));

    let name = match (is_private, final_byte) {
        (true, 'h') => "DECSET",
        (true, 'l') => "DECRST",
        (true, 's') => "DECSCP",
        (true, 'r') => "DECSTR",
        (_, '@') => "ICH",
        (_, 'A') => "CUU",
        (_, 'B') => "CUD",
        (_, 'C') => "CUF",
        (_, 'D') => "CUB",
        (_, 'E') => "CNL",
        (_, 'F') => "CPL",
        (_, 'G') => "CHA",
        (_, 'H' | 'f') => "CUP",
        (_, 'J') => "ED",
        (_, 'K') => "EL",
        (_, 'L') => "IL",
        (_, 'M') => "DL",
        (_, 'P') => "DCH",
        (_, 'S') => "SU",
        (_, 'T') => "SD",
        (_, 'X') => "ECH",
        (_, 'd') => "VPA",
        (_, 'e') => "VPR",
        (_, 'g') => "TBC",
        (_, 'h') => "SM",
        (_, 'l') => "RM",
        (_, 'm') => "SGR",
        (_, 'n') => "DSR",
        (_, 'q') => "DECLL",
        (_, 'r') => "DECSTBM",
        (_, 's') => "SCP",
        (_, 't') => "WindowOps",
        (_, 'u') => "RCP",
        _ => {
            if is_private {
                "DEC-CSI"
            } else {
                "CSI"
            }
        }
    };

    format!("<ESC[{} {}>", sequence, name)
}

fn format_escape_sequence(next: char) -> String {
    let name = match next {
        'D' => "IND",
        'E' => "NEL",
        'H' => "HTS",
        'M' => "RI",
        'N' => "SS2",
        'O' => "SS3",
        '7' => "DECSC",
        '8' => "DECRC",
        '=' => "DECKPAM",
        '>' => "DECKPNM",
        'c' => "RIS",
        _ => return format!("<ESC {}>", next),
    };

    format!("<ESC {}>", name)
}

fn format_charset_sequence(intermediate: char, designator: char) -> String {
    let slot = match intermediate {
        '(' => "G0",
        ')' => "G1",
        '*' => "G2",
        '+' => "G3",
        '-' => "G1",
        '.' => "G2",
        '/' => "G3",
        _ => "G?",
    };

    let charset = match designator {
        '0' => "DEC Special Graphics",
        'A' => "UK",
        'B' => "ASCII",
        '4' => "Dutch",
        'C' | '5' => "Finnish",
        'R' => "French",
        'Q' => "French Canadian",
        'K' => "German",
        'Y' => "Italian",
        'E' | '6' => "Norwegian/Danish",
        'Z' => "Spanish",
        'H' | '7' => "Swedish",
        '=' => "Swiss",
        _ => return format!("<ESC{}{} Charset>", intermediate, designator),
    };

    format!(
        "<ESC{}{} {} {} Charset>",
        intermediate, designator, slot, charset
    )
}

fn control_name(ch: char) -> Option<&'static str> {
    match ch {
        '\0' => Some("NUL"),
        '\u{1}' => Some("SOH"),
        '\u{2}' => Some("STX"),
        '\u{3}' => Some("ETX"),
        '\u{4}' => Some("EOT"),
        '\u{5}' => Some("ENQ"),
        '\u{6}' => Some("ACK"),
        '\u{7}' => Some("BEL"),
        '\u{b}' => Some("VT"),
        '\u{c}' => Some("FF"),
        '\u{e}' => Some("SO"),
        '\u{f}' => Some("SI"),
        '\u{10}' => Some("DLE"),
        '\u{11}' => Some("DC1"),
        '\u{12}' => Some("DC2"),
        '\u{13}' => Some("DC3"),
        '\u{14}' => Some("DC4"),
        '\u{15}' => Some("NAK"),
        '\u{16}' => Some("SYN"),
        '\u{17}' => Some("ETB"),
        '\u{18}' => Some("CAN"),
        '\u{19}' => Some("EM"),
        '\u{1a}' => Some("SUB"),
        '\u{1c}' => Some("FS"),
        '\u{1d}' => Some("GS"),
        '\u{1e}' => Some("RS"),
        '\u{1f}' => Some("US"),
        '\u{7f}' => Some("DEL"),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::format_terminal_output_for_preview;

    #[test]
    fn renders_ansi_sequences_as_readable_markers() {
        let content = "\u{1b}[31merror\u{1b}[0m: failed\n\u{1b}]0;title\u{7}done";

        assert_eq!(
            format_terminal_output_for_preview(content),
            "<ESC[31m SGR>error<ESC[0m SGR>: failed\n<OSC 0;title BEL>done"
        );
    }

    #[test]
    fn renders_carriage_return_as_marker() {
        let content = "Downloading 10%\rDownloading 100%\nnext";

        assert_eq!(
            format_terminal_output_for_preview(content),
            "Downloading 10%<CR>Downloading 100%\nnext"
        );
    }

    #[test]
    fn renders_backspace_as_marker() {
        let content = "abc\u{8}\u{8}12";

        assert_eq!(format_terminal_output_for_preview(content), "abc<BS><BS>12");
    }

    #[test]
    fn renders_cross_platform_vt_sequences_readably() {
        let content = "\u{1b}[?25l\u{1b}[2K\rprogress\u{1b}[?25h";

        assert_eq!(
            format_terminal_output_for_preview(content),
            "<ESC[?25l DECRST><ESC[2K EL><CR>progress<ESC[?25h DECSET>"
        );
    }

    #[test]
    fn renders_single_character_escape_sequences() {
        let content = "\u{1b}7saved\u{1b}8\u{1b}c";

        assert_eq!(
            format_terminal_output_for_preview(content),
            "<ESC DECSC>saved<ESC DECRC><ESC RIS>"
        );
    }

    #[test]
    fn renders_charset_designation_sequences() {
        let content = "\u{1b}(0line";

        assert_eq!(
            format_terminal_output_for_preview(content),
            "<ESC(0 G0 DEC Special Graphics Charset>line"
        );
    }
}
