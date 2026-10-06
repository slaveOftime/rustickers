//! Native clipboard capture, internal-write suppression, and verified paste adapters.

pub mod paste;
pub mod watcher;

pub use paste::{foreground, paste_foreground};
pub use watcher::{CaptureFilter, CaptureSource, CapturedText, write_clipboard_text};
