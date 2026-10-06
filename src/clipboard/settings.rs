//! User-tunable limits for the clipboard collector.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardSettings {
    /// Maximum number of *ordinary* rows. Retained rows do not count.
    pub normal_count_limit: i64,
    /// Maximum total bytes of *ordinary* content. Retained rows do not count.
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
