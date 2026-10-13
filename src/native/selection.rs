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
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use crate::native::clipboard::CaptureFilter;

#[cfg(target_os = "windows")]
mod windows_capture;

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

/// Same as [`capture_selection`] with the history watcher's internal-write filter.
/// This is blocking input/clipboard work; UI callers must run it on a worker so
/// the foreground application can process the simulated copy.
pub fn capture_selection_with_filter(
    filter: &Arc<CaptureFilter>,
) -> anyhow::Result<Option<String>> {
    capture_selection_from_window(filter, foreground_window())
}

pub(super) fn foreground_window() -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        let hwnd = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
        (!hwnd.is_null()).then_some(hwnd as u64)
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

fn verify_foreground(expected: Option<u64>) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    anyhow::ensure!(
        expected.is_some() && foreground_window() == expected,
        "Selection source lost focus; select the text and retry."
    );
    #[cfg(not(target_os = "windows"))]
    let _ = expected;
    Ok(())
}

pub(super) fn capture_selection_from_window(
    filter: &Arc<CaptureFilter>,
    source: Option<u64>,
) -> anyhow::Result<Option<String>> {
    // Desktop focus and a native edit's collapsed selection are definitive:
    // return before waiting for modifiers or touching the clipboard. Unknown
    // apps retain the full asynchronous Copy timeout for compatibility.
    #[cfg(target_os = "windows")]
    if windows_capture::known_empty_selection(source) {
        return Ok(None);
    }
    // SendInput does not neutralize physically held modifiers. In particular,
    // Ctrl+Shift+Space must not turn our Ctrl+C into Ctrl+Shift+C. Never fake
    // releases of keys still held by the user.
    #[cfg(target_os = "windows")]
    crate::native::clipboard::paste::wait_for_modifiers()
        .context("Release selection shortcut modifiers and retry")?;
    let _transaction = filter.transaction();
    verify_foreground(source)?;
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

    let copy_result = verify_foreground(source).and_then(|_| {
        dispatch_copy(primary, |event| {
            simulate(&event).map_err(|err| anyhow::anyhow!("failed to copy selected text: {err:?}"))
        })
    });
    if let Err(err) = copy_result {
        restore_clipboard(&mut clipboard, previous_text.as_deref(), filter);
        return Err(err);
    }

    // Copy can be asynchronous (browser/Electron apps and remote sessions).
    // Keep the UI pumping while we wait, and do not confuse the old clipboard
    // content with a captured selection when the app does not implement Copy.
    let mut text = None;
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        thread::sleep(Duration::from_millis(20));
        if let Ok(current) = clipboard.get_text()
            && current != marker
        {
            text = Some(current);
            break;
        }
        if Instant::now() >= deadline {
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

fn dispatch_copy(
    primary: Key,
    mut send: impl FnMut(EventType) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let press =
        send(EventType::KeyPress(primary)).and_then(|_| send(EventType::KeyPress(Key::KeyC)));
    let release_c = send(EventType::KeyRelease(Key::KeyC));
    let release_primary = send(EventType::KeyRelease(primary));
    press.and(release_c).and(release_primary)
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

#[cfg(all(test, target_os = "windows"))]
mod windows_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_copy_still_releases_synthetic_keys() {
        let mut events = Vec::new();
        let result = dispatch_copy(Key::ControlLeft, |event| {
            events.push(event);
            anyhow::ensure!(events.len() != 2, "injected copy failure");
            Ok(())
        });
        assert!(result.is_err());
        assert!(matches!(events[2], EventType::KeyRelease(Key::KeyC)));
        assert!(matches!(events[3], EventType::KeyRelease(Key::ControlLeft)));
    }

    #[test]
    fn copy_reports_release_failure() {
        let mut calls = 0;
        assert!(
            dispatch_copy(Key::ControlLeft, |_| {
                calls += 1;
                anyhow::ensure!(calls != 3, "injected release failure");
                Ok(())
            })
            .is_err()
        );
        assert_eq!(calls, 4);
    }

    #[test]
    fn service_constructs_with_a_filter() {
        let filter = CaptureFilter::new();
        let service = SelectionCaptureService::new(filter);
        // Calling capture without a real keyboard would still try to simulate Ctrl+C. Skip the
        // interactive part — we just check the type can be moved and the filter is reachable.
        let _ = service;
    }
}
