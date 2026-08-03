//! Cross-platform clipboard-based selection capture.

use anyhow::Context;
use rdev::{EventType, Key, simulate};
use std::{thread, time::Duration};

/// Copy and read the current text selection from the active application.
///
/// Returns `Ok(None)` when the active application has no direct text
/// selection. The previous clipboard content is never used as a fallback,
/// callers are expected to ask the user for the text instead.
pub fn capture_selection() -> anyhow::Result<Option<String>> {
    let mut clipboard = arboard::Clipboard::new().context("failed to open system clipboard")?;
    let previous_text = clipboard.get_text().ok();
    let marker = format!(
        "__RUSTICKERS_SELECTION_{}__",
        crate::utils::time::now_unix_millis()
    );
    if let Err(err) = clipboard.set_text(marker.clone()) {
        return Err(anyhow::anyhow!(
            "failed to prepare clipboard for selection capture: {err}"
        ));
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

    restore_clipboard(&mut clipboard, previous_text.as_deref());
    Ok(match text {
        Some(text) if !text.trim().is_empty() => Some(text),
        _ => None,
    })
}

fn restore_clipboard(clipboard: &mut arboard::Clipboard, previous_text: Option<&str>) {
    if let Some(previous_text) = previous_text {
        let _ = clipboard.set_text(previous_text);
    } else {
        let _ = clipboard.clear();
    }
}
