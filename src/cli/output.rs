//! How the CLI talks back.
//!
//! Every command speaks two languages. By default it writes a compact, aligned report meant to be
//! read by a person. With `--json` it writes exactly one JSON object to stdout and nothing else,
//! so a script or an agent can parse the result without scraping prose.
//!
//! The JSON object always carries an `ok` field, which means a caller can branch on the payload
//! alone and never has to reason about exit codes and stderr at the same time.

use serde_json::{Map, Value, json};

/// Which language the CLI is speaking for this invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Human,
    Json,
}

impl Format {
    pub fn new(json: bool) -> Self {
        if json { Self::Json } else { Self::Human }
    }

    pub fn is_json(self) -> bool {
        self == Self::Json
    }

    /// Print a successful result: the JSON payload or, for humans, whatever `human` writes.
    ///
    /// The human branch is a closure so the (often costly) formatting never runs in JSON mode.
    pub fn emit(self, data: Value, human: impl FnOnce()) {
        match self {
            Self::Json => println!("{}", success(data)),
            Self::Human => human(),
        }
    }

    /// Print a failure. Humans get it on stderr, JSON callers get `{"ok": false, ...}` on stdout
    /// alongside every other reply, so one parser handles both outcomes.
    pub fn emit_error(self, err: &anyhow::Error) {
        match self {
            Self::Json => println!("{}", json!({ "ok": false, "error": format!("{err:#}") })),
            Self::Human => eprintln!("error: {err:#}"),
        }
    }

    /// A remark that is worth reading but is not the result: a hint, a caveat, a next step.
    ///
    /// Silent in JSON mode, because the same information belongs in the payload where a machine
    /// can actually act on it.
    pub fn note(self, message: impl std::fmt::Display) {
        if self == Self::Human {
            println!("{message}");
        }
    }
}

/// Wrap a payload in the standard success envelope.
fn success(data: Value) -> Value {
    let mut map = match data {
        Value::Object(map) => map,
        other => {
            let mut map = Map::new();
            map.insert("data".into(), other);
            map
        }
    };
    map.insert("ok".into(), Value::Bool(true));
    Value::Object(map)
}

/// Shorten a string to `max` characters, appending `…` when anything was dropped.
///
/// Counts characters rather than bytes, so it never splits a multi-byte character.
pub fn ellipsize(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Collapse a string onto one line so it can sit inside a table cell.
pub fn one_line(s: &str, max: usize) -> String {
    let flattened = s.split_whitespace().collect::<Vec<_>>().join(" ");
    ellipsize(&flattened, max)
}

pub fn format_ts(ts: i64) -> String {
    use chrono::{DateTime, Utc};
    DateTime::from_timestamp_millis(ts)
        .map(|dt: DateTime<Utc>| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| ts.to_string())
}

/// Render rows under a header, sizing every column to its widest cell.
///
/// A fixed-width layout wastes space on short lists and truncates long ids on big ones, so the
/// widths are measured instead of guessed.
pub fn table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(i) {
                *width = (*width).max(cell.chars().count());
            }
        }
    }

    let line = |cells: &[String]| {
        let mut out = String::new();
        for (i, cell) in cells.iter().enumerate() {
            let is_last = i + 1 == cells.len();
            if is_last {
                out.push_str(cell);
            } else {
                let pad = widths.get(i).copied().unwrap_or(0) - cell.chars().count();
                out.push_str(cell);
                out.push_str(&" ".repeat(pad + 2));
            }
        }
        println!("{out}");
    };

    line(&headers.iter().map(|h| (*h).to_owned()).collect::<Vec<_>>());
    let rule: usize = widths.iter().sum::<usize>() + 2 * widths.len().saturating_sub(1);
    println!("{}", "─".repeat(rule));
    for row in rows {
        line(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipsize_never_splits_a_multi_byte_character() {
        assert_eq!(ellipsize("日本語のテキスト", 4), "日本語の…");
        assert_eq!(ellipsize("héllo", 5), "héllo");
        assert_eq!(ellipsize("héllo world", 5), "héllo…");
    }

    #[test]
    fn ellipsize_leaves_short_strings_untouched() {
        assert_eq!(ellipsize("abc", 10), "abc");
        assert_eq!(ellipsize("", 3), "");
    }

    #[test]
    fn one_line_flattens_whitespace() {
        assert_eq!(one_line("a\n  b\tc  ", 40), "a b c");
    }

    #[test]
    fn the_success_envelope_keeps_the_payload_fields() {
        let value = success(json!({ "id": 7 }));
        assert_eq!(value["ok"], json!(true));
        assert_eq!(value["id"], json!(7));
    }

    #[test]
    fn a_non_object_payload_is_nested_under_data() {
        let value = success(json!([1, 2]));
        assert_eq!(value["ok"], json!(true));
        assert_eq!(value["data"], json!([1, 2]));
    }
}
