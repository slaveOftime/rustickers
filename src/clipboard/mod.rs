//! Shared clipboard history model.
//!
//! Both the GUI chooser and the CLI `cp ls` command read, filter, and promote rows through the
//! functions here, so two surfaces never disagree about what "recent" or "matched by query"
//! means. Storage backends implement [`storage::ClipboardHistoryStore`]; the data model itself
//! stays in this module so tests can exercise it without a database.

pub mod argv;
pub mod filter;
pub mod items;
pub mod ordering;
pub mod retention;
pub mod settings;

#[cfg(feature = "storage")]
pub mod storage;

#[cfg(feature = "storage")]
pub mod vault;
#[cfg(feature = "storage")]
pub mod vault_controller;

#[cfg(test)]
mod tests;

pub use items::{
    ClipboardItem, ClipboardItemKind, PROTECTED_MATCH_ABORT_RUN, ProtectedPreview, RetentionState,
};

pub use settings::ClipboardSettings;
