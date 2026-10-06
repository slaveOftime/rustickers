//! Cross-platform clipboard-based selection capture.
//!
//! The capture helper used to communicate directly with the arboard clipboard, but as soon as the
//! clipboard history collector came online (M1+) it became responsible for flipping an internal
//! flag so the watcher does not stash the temporary marker we drop on the clipboard.
//!
//! Callers that need this protection construct a [`selection_with_filter`] closure via
//! [`SelectionCaptureService::new`], which holds the [`CaptureFilter`] the watcher has been
//! built with. In tests and CLIs without a watcher the helper still works because the default
//! is "always treat the write as internal" via a dedicated filter.

use anyhow::Context;
use rdev::{EventType, Key, simulate};
use std::{sync::Arc, thread, time::Duration};

use crate::native::clipboard::CaptureFilter;

/// A self-contained handle on the selection helper.
///
/// The single concrete type keeps tests, the GUI bootstrap, and any future IPC recipient all on
/// the same surface. The default constructor uses an isolated filter so writes are never
/// delivered to any running watcher.
pub struct SelectionCaptureService {
    filter: Arc<CaptureFilter>,
}

impl SelectionCaptureService {
    pub fn new(filter: Arc<CaptureFilter>) -> Self {
        Self { filter }
    }

    /// Read the currently focused foreground application's text selection.
    pub fn capture_selection(&self) -> anyhow::Result<Option<String>> {
        let filter = self.filter.clone();
        capture_selection_with_filter(&filter)
    }
}

impl Default for SelectionCaptureService {
    fn default() -> Self {
        Self {
            filter: CaptureFilter::new(),
        }
    }
}

/// Copy and read the current text selection from the active application.
///
/// Returns `Ok(None)` when the active application has no direct text selection. The previous
/// clipboard content is restored afterwards. The temporary marker and the restore step go
/// through [`CaptureFilter::note_internal_write`] so the history collector never ingests them.
pub fn capture_selection() -> anyhow::Result<Option<String>> {
    capture_selection_with_filter(&CaptureFilter::new())
}

/// Same as [`capture_selection`] but reusable across calls when a long-lived
/// [`CaptureFilter`] is in scope. The filter is notified twice per call: once for the marker,
/// and once for the restore. Both writes pair up with a single
/// [`watcher::WatcherHandle::spawn`] tick so no keyboard events are missed.
pub fn capture_selection_with_filter(
    filter: &Arc<CaptureFilter>,
) -> anyhow::Result<Option<String>> {
    let _transaction = filter.transaction();
    let mut clipboard = arboard::Clipboard::new().context("failed to open system clipboard")?;
    let previous_text = clipboard.get_text().ok();
    let marker = format!(
        "__RUSTICKERS_SELECTION_{}__",
        crate::utils::time::now_unix_millis()
    );
    filter.note_internal_write();
    if let Err(err) = clipboard.set_text(marker.clone()) {
        return Err(anyhow::anyhow!(
            "failed to prepare clipboard for selection capture: {err}"
        ));
    }

    filter.record_internal_generation();
    #[cfg(target_os = "macos")]
    let primary = Key::MetaLeft;
    #[cfg(not(target_os = "macos"))]
    let primary = Key::ControlLeft;

    let copy_result = simulate(&EventType::KeyPress(primary))
        .and_then(|_| simulate(&EventType::KeyPress(Key::KeyC)));
    let _ = simulate(&EventType::KeyRelease(Key::KeyC));
    let _ = simulate(&EventType::KeyRelease(primary));
    if let Err(err) = copy_result {
        restore_clipboard(&mut clipboard, previous_text.as_deref(), filter);
        return Err(anyhow::anyhow!("failed to copy selected text: {err:?}"));
    }

    let mut text = None;
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(20));
        if let Ok(current) = clipboard.get_text()
            && current != marker
        {
            text = Some(current);
            break;
        }
    }

    if clipboard.get_text().ok().as_deref() == text.as_deref().or(Some(marker.as_str())) {
        restore_clipboard(&mut clipboard, previous_text.as_deref(), filter);
    }
    Ok(match text {
        Some(text) if !text.trim().is_empty() => Some(text),
        _ => None,
    })
}

fn restore_clipboard(
    clipboard: &mut arboard::Clipboard,
    previous_text: Option<&str>,
    filter: &Arc<CaptureFilter>,
) {
    filter.note_internal_write();
    if let Some(previous_text) = previous_text {
        let _ = clipboard.set_text(previous_text);
    } else {
        let _ = clipboard.clear();
    }
    filter.record_internal_generation();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_constructs_with_a_filter() {
        let filter = CaptureFilter::new();
        let service = SelectionCaptureService::new(filter);
        // Calling capture without a real keyboard would still try to simulate Ctrl+C. Skip the
        // interactive part — we just check the type can be moved and the filter is reachable.
        let _ = service;
    }
}
