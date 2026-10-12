//! Shared text collection: ordinary Markdown stickers and disposable captures.
//! Mutation and protection policy live here, not in picker adapters.
pub mod protection;
mod repository;
pub use repository::{CaptureOutcome, TextRepository};
pub(crate) use repository::{absorb_captures, digest, reconcile_captures};

use anyhow::{Result, bail};
use serde::Serialize;
use std::{fmt, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(into = "String")]
pub enum TextEntryId {
    Sticker(i64),
    Clipboard(i64),
}
impl fmt::Display for TextEntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sticker(id) => write!(f, "sticker:{id}"),
            Self::Clipboard(id) => write!(f, "clipboard:{id}"),
        }
    }
}
impl From<TextEntryId> for String {
    fn from(id: TextEntryId) -> Self {
        id.to_string()
    }
}
impl FromStr for TextEntryId {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        let (source, number) = value
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("use sticker:<ID> or clipboard:<ID>"))?;
        let id: i64 = number
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid text ID"))?;
        anyhow::ensure!(id > 0, "text ID must be positive");
        match source {
            "sticker" => Ok(Self::Sticker(id)),
            "clipboard" => Ok(Self::Clipboard(id)),
            _ => bail!("unknown text source"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TextSources {
    stickers: bool,
    clipboard: bool,
}
impl TextSources {
    pub fn new(stickers: bool, clipboard: bool) -> Self {
        Self {
            stickers,
            clipboard,
        }
    }
    pub fn includes(self, id: TextEntryId) -> bool {
        match id {
            TextEntryId::Sticker(_) => self.stickers,
            TextEntryId::Clipboard(_) => self.clipboard,
        }
    }
}

/// Redacted public snapshot. Ciphertext and full searchable text are not serialized.
#[derive(Clone, Serialize)]
pub struct TextEntry {
    id: TextEntryId,
    title: String,
    preview: String,
    encrypted: bool,
    state: Option<crate::model::sticker::StickerState>,
    created_at: i64,
    activity_at: i64,
    revision: i64,
    /// SHA-256 of the underlying plaintext bytes, lowercase hex. Empty for
    /// rows whose content is not yet digested (legacy entries that were never
    /// edited through the new repository paths). Always set for new captures,
    /// edits, lock transitions, and promotions. Internal only: public snapshots
    /// must not expose fingerprints of locked content.
    #[serde(skip)]
    digest: String,
    #[serde(skip)]
    content: String,
}
impl fmt::Debug for TextEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextEntry")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}
impl TextEntry {
    pub fn id(&self) -> TextEntryId {
        self.id
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn preview(&self) -> &str {
        &self.preview
    }
    /// Bounded multiline display text. Locked entries expose only their public title.
    pub fn multiline_preview(&self) -> String {
        let text = if self.encrypted {
            &self.title
        } else {
            &self.content
        };
        text.chars().take(1024).collect()
    }
    pub fn is_encrypted(&self) -> bool {
        self.encrypted
    }
    pub fn state(&self) -> Option<crate::model::sticker::StickerState> {
        self.state
    }
    pub fn revision(&self) -> i64 {
        self.revision
    }
    pub fn activity_at(&self) -> i64 {
        self.activity_at
    }
    pub fn matches(&self, query: &str) -> bool {
        let searchable = if self.encrypted {
            format!("{} {}", self.title, self.id)
        } else {
            format!("{} {}", self.title, self.content)
        }
        .to_lowercase();
        query
            .split_whitespace()
            .all(|token| searchable.contains(&token.to_lowercase()))
    }
}

pub fn filter_indices(entries: &[TextEntry], query: &str) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(ix, entry)| entry.matches(query).then_some(ix))
        .collect()
}
pub fn compare_by_recency(a: &TextEntry, b: &TextEntry) -> std::cmp::Ordering {
    fn identity(id: TextEntryId) -> (u8, i64) {
        match id {
            TextEntryId::Sticker(n) => (0, n),
            TextEntryId::Clipboard(n) => (1, n),
        }
    }
    let (a_source, a_id) = identity(a.id);
    let (b_source, b_id) = identity(b.id);
    b.activity_at
        .cmp(&a.activity_at)
        .then_with(|| b.created_at.cmp(&a.created_at))
        .then_with(|| a_source.cmp(&b_source))
        .then_with(|| b_id.cmp(&a_id))
}
