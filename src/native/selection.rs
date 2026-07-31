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
    clipboard
        .set_text(marker.clone())
        .context("failed to prepare clipboard for selection capture")?;

    #[cfg(target_os = "macos")]
    let primary = Key::MetaLeft;
    #[cfg(not(target_os = "macos"))]
    let primary = Key::ControlLeft;

    let copy_result = simulate(&EventType::KeyPress(primary))
        .and_then(|_| simulate(&EventType::KeyPress(Key::KeyC)));
    let _ = simulate(&EventType::KeyRelease(Key::KeyC));
    let _ = simulate(&EventType::KeyRelease(primary));
    if let Err(err) = copy_result {
        restore_clipboard(&mut clipboard, previous_text);
        anyhow::bail!("failed to copy selected text: {err:?}");
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

    let Some(text) = text else {
        restore_clipboard(&mut clipboard, previous_text);
        anyhow::bail!("no text is selected in the active application");
    };
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        restore_clipboard(&mut clipboard, previous_text);
        anyhow::bail!("selected text is empty");
    }
    Ok(trimmed)
}

fn restore_clipboard(clipboard: &mut arboard::Clipboard, previous_text: Option<String>) {
    if let Some(previous_text) = previous_text {
        let _ = clipboard.set_text(previous_text);
    } else {
        let _ = clipboard.clear();
    }
}
