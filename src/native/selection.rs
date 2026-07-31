//! Cross-platform clipboard-based selection capture.

use anyhow::Context;
use rdev::{EventType, Key, simulate};
use std::{thread, time::Duration};

/// Copy and read the current text selection from the active application.
pub fn capture_selection() -> anyhow::Result<String> {
    let mut clipboard = arboard::Clipboard::new().context("failed to open system clipboard")?;
    let previous_text = clipboard.get_text().ok();
    let marker = format!(
        "__RUSTICKERS_SELECTION_{}__",
        crate::utils::time::now_unix_millis()
    );
    if let Err(err) = clipboard.set_text(marker.clone()) {
        return previous_clipboard_or_error(
            previous_text,
            format!("failed to prepare clipboard for selection capture: {err}"),
        );
    }

    #[cfg(target_os = "macos")]
    let primary = Key::MetaLeft;
    #[cfg(not(target_os = "macos"))]
    let primary = Key::ControlLeft;

    let copy_result = simulate(&EventType::KeyPress(primary))
        .and_then(|_| simulate(&EventType::KeyPress(Key::KeyC)));
    let _ = simulate(&EventType::KeyRelease(Key::KeyC));
    let _ = simulate(&EventType::KeyRelease(primary));
    if let Err(err) = copy_result {
        restore_clipboard(&mut clipboard, previous_text.as_deref());
        return previous_clipboard_or_error(
            previous_text,
            format!("failed to copy selected text: {err:?}"),
        );
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

    restore_clipboard(&mut clipboard, previous_text.as_deref());
    match text {
        Some(text) if !text.trim().is_empty() => Ok(text),
        Some(_) => previous_clipboard_or_error(previous_text, "selected text is empty"),
        None => previous_clipboard_or_error(
            previous_text,
            "no text is selected in the active application",
        ),
    }
}

fn previous_clipboard_or_error(
    previous_text: Option<String>,
    error: impl Into<String>,
) -> anyhow::Result<String> {
    match previous_text {
        Some(text) if !text.trim().is_empty() => Ok(text),
        _ => Err(anyhow::anyhow!(error.into())),
    }
}

fn restore_clipboard(clipboard: &mut arboard::Clipboard, previous_text: Option<&str>) {
    if let Some(previous_text) = previous_text {
        let _ = clipboard.set_text(previous_text);
    } else {
        let _ = clipboard.clear();
    }
}
