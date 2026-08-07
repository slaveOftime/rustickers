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

/// Written into a command sticker's arguments, and replaced with the captured text when the
/// sticker is launched from a text selection. Only arguments are substituted, never the program.
pub const SELECTION_PLACEHOLDER: &str = "{{RUSTICKERS_SELECTION}}";

/// The captured text is also handed to every selection run through this environment variable, so
/// a command can read the selection without using the placeholder at all.
pub const SELECTION_ENV_VAR: &str = "RUSTICKERS_SELECTION";

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

impl CommandResult {
    /// The rendered payload, whatever shape the output is rendered in.
    pub fn value(&self) -> Option<&String> {
        match self {
            Self::Text(value)
            | Self::Html(value)
            | Self::Svg(value)
            | Self::Markdown(value)
            | Self::Source(value) => value.as_ref(),
        }
    }

    /// Replace the payload, keeping the render mode the user chose.
    pub fn set(&mut self, payload: Option<String>) {
        match self {
            Self::Text(value)
            | Self::Html(value)
            | Self::Svg(value)
            | Self::Markdown(value)
            | Self::Source(value) => *value = payload,
        }
    }

    pub fn clear(&mut self) {
        self.set(None);
    }
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
