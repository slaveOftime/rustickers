//! The clipboard history data model.
//!
//! A [`ClipboardItem`] captures everything the GUI chooser and CLI need to share: stable identity,
//! recency, retention reasons, and the actual content (either plain text or a protected envelope).
//! Pure functions like [`recency_at_ms`] and [`retention_state`] derive the values that drive both
//! the shared ordering and the retention policy.

use serde::{Deserialize, Serialize};

/// A sentinel returned by [`recency_at_ms`] when the item is currently being edited or used, and
/// the capture cycle should drop the new copy rather than evict the in-use one. Callers that
/// return this value skip pruning only if there are no other ordinary victims available.
pub const PROTECTED_MATCH_ABORT_RUN: i64 = i64::MIN;

/// The kind of content stored in a single clipboard row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardItemKind {
    /// Plain text that is collected automatically and may be searched.
    Plain,
    /// Text encrypted with the master's data key; only metadata is searchable.
    Protected,
}

/// What an ordered row looks like to the chooser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardItem {
    pub id: i64,
    pub kind: ClipboardItemKind,
    pub content: String,
    pub plain_preview: Option<String>,
    pub public_label: Option<String>,
    pub byte_size: i64,
    pub created_at: i64,
    pub last_copied_at: i64,
    pub last_used_at: Option<i64>,
    pub last_edited_at: Option<i64>,
    pub retention: RetentionState,
}

/// Empty rows must never be saved: capture only stores rows that contain at least one
/// non-whitespace character. White-space-only content is also rejected so callers cannot trick the
/// store into recording empty boards.
pub fn is_collectable(content: &str) -> bool {
    !content.is_empty() && content.chars().any(|c| !c.is_whitespace())
}

/// The single MRU timestamp that drives shared ordering. Comparisons should never enumerate every
/// field by hand; this function is the rule. It uses `MAX(...)` of:
/// - `created_at` — when the row first appeared.
/// - `last_copied_at` — when the same content was last seen copied while the user is not
///   actively editing.
/// - `last_used_at` — when the row was last successfully pasted or run.
/// - `last_edited_at` — when a saved edit changed the row's content.
///
/// Reserve/protect/unlock alone do not bump this value. Promotion happens after the action
/// completes, never while the user is still choosing what to do.
pub fn recency_at_ms(item: &ClipboardItem) -> i64 {
    item.created_at
        .max(item.last_copied_at)
        .max(item.last_used_at.unwrap_or(i64::MIN))
        .max(item.last_edited_at.unwrap_or(i64::MIN))
}

/// The retention reasons attached to a row are independent flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionState {
    pub explicit: bool,
    pub edited: bool,
    pub protected_: bool,
}

impl RetentionState {
    pub fn is_retained(&self) -> bool {
        self.explicit || self.edited || self.protected_
    }

    pub fn ordinary(&self) -> bool {
        !self.is_retained()
    }

    pub fn explicit(&self) -> bool {
        self.explicit
    }

    pub fn edited(&self) -> bool {
        self.edited
    }

    pub fn protected(&self) -> bool {
        self.protected_
    }
}

impl ClipboardItem {
    /// The human-readable text used both for filtering and as the row label.
    ///
    /// For ordinary items this is the row's content. For protected items the chooser never has
    /// the plaintext unless it is unlocked. Only an explicitly public alias or generic label
    /// is displayed; previews and ciphertext are never used as protected display text.
    pub fn preview_text(&self) -> String {
        if let Some(public_label) = self.public_label.as_ref().filter(|s| !s.is_empty()) {
            return public_label.clone();
        }
        if self.kind == ClipboardItemKind::Protected {
            return "Protected item".to_string();
        }
        if let Some(preview) = self.plain_preview.as_ref().filter(|p| !p.is_empty()) {
            // Collapse newlines so each entry renders as a single row, then truncate.
            let collapsed: String = preview
                .chars()
                .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
                .collect();
            return collapsed.chars().take(120).collect();
        }
        if !self.content.is_empty() {
            return self.content.chars().take(120).collect();
        }
        self.id.to_string()
    }

    /// The string the chooser feeds into [`crate::clipboard::filter::filter_indices`]. We must
    /// never include the secret plaintext of a protected row because the chooser rebuilds this
    /// string before the vault is opened.
    pub fn searchable_string(&self) -> String {
        let mut combined = String::new();
        if let Some(label) = &self.public_label {
            combined.push_str(label);
            combined.push('\n');
        }
        if self.kind == ClipboardItemKind::Protected {
            return combined;
        }
        if let Some(preview) = &self.plain_preview {
            combined.push_str(preview);
        }
        if combined.is_empty() {
            combined.push_str(&self.content);
        }
        combined
    }
}

/// What the chooser surfaces about a protected row. The plaintext is the value just authorised by
/// the master password; the `recency_at_ms` view is unchanged so the picker still shows it in MRU
/// order. The match token, when present, lets collection replace a fresh identical copy rather
/// than create a parallel ordinary row.
#[derive(Clone)]
pub struct ProtectedPreview {
    pub item: ClipboardItem,
    pub plaintext: String,
    pub match_token: Option<String>,
}

impl std::fmt::Debug for ProtectedPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtectedPreview")
            .field("id", &self.item.id)
            .field("plaintext", &"[redacted]")
            .field("match_token", &"[redacted]")
            .finish()
    }
}
