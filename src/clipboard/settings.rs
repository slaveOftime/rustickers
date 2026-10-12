//! User-tunable limits for the clipboard collector.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardSettings {
    /// Maximum disposable clipboard captures. Saved stickers never count.
    pub normal_count_limit: i64,
    /// Maximum total UTF-8 bytes of disposable captures. Saved stickers never count.
    pub normal_byte_limit: i64,
    /// Whether the desktop collector is allowed to ingest.
    pub capture_enabled: bool,
}

impl Default for ClipboardSettings {
    fn default() -> Self {
        Self {
            normal_count_limit: 500,
            normal_byte_limit: 20 * 1024 * 1024,
            capture_enabled: false,
        }
    }
}
