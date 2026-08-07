use serde::{Deserialize, Serialize};

// ── File sticker ─────────────────────────────────────────────────────────────

const EMPTY_FILE_LIST_JSON: &str = "{\"files\":[]}";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStickerContent {
    pub files: Vec<String>,
}

impl FileStickerContent {
    pub fn from_sources(sources: &[String]) -> Self {
        Self {
            files: sources.to_vec(),
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

// ── Command sticker ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandContent {
    pub command: String,
    pub environments: String,
    pub working_dir: String,
    pub scheduler: Option<Scheduler>,
    pub run_immediately: bool,
    pub result: CommandResult,
    pub stream_result: bool,
    pub padding: Option<u8>,
    pub started_at: Option<i64>,
    #[serde(default)]
    pub accept_selection: bool,
    #[serde(default)]
    pub auto_close: bool,
    #[serde(default)]
    pub run_without_window: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Scheduler {
    Cron(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandResult {
    Text(Option<String>),
    Html(Option<String>),
    Svg(Option<String>),
    Markdown(Option<String>),
    Source(Option<String>),
}

impl Default for CommandContent {
    fn default() -> Self {
        Self {
            command: String::new(),
            environments: String::new(),
            working_dir: String::new(),
            scheduler: None,
            run_immediately: true,
            stream_result: false,
            result: CommandResult::Text(None),
            padding: None,
            started_at: None,
            accept_selection: false,
            auto_close: false,
            run_without_window: false,
        }
    }
}
