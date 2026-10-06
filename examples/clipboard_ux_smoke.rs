//! Isolated visual smoke harness: fixture database only, no watcher or clipboard writes.
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, application};
use rustickers::clipboard::storage::{ClipboardHistoryStore, SqliteClipboardStore};
use rustickers::clipboard::vault_controller::VaultController;
use rustickers::native::{clipboard::CaptureFilter, components::Assets, windows::clipboard};
use std::sync::{Arc, mpsc};

fn main() -> anyhow::Result<()> {
    let fixtures = tempfile::tempdir()?;
    let store = Arc::new(futures::executor::block_on(SqliteClipboardStore::open(
        fixtures.path().join("clipboard.db"),
    ))?);
    for (index, text) in [
        "A short clipboard item",
        "https://example.test/docs/clipboard-history?view=keyboard",
        "日本語 · 中文 · emoji ✨",
        "A multiline copy\nwith a second line",
        "Another recent copy",
        "Code: let greeting = \"Hello, world!\";",
        "A useful saved command",
        "The oldest sample",
    ]
    .iter()
    .enumerate()
    {
        futures::executor::block_on(store.upsert_ordinary(
            text,
            text,
            &SqliteClipboardStore::ordinary_digest(text),
            text.len() as i64,
            100 - index as i64,
        ))?;
    }
    let vault = Arc::new(VaultController::new(store.clone()));
    // Default exercises first-time setup; --configured exercises existing-vault unlock.
    if std::env::args().any(|arg| arg == "--configured") {
        futures::executor::block_on(vault.create("fixture-password"))?;
    }
    let app = application().with_assets(Assets);
    app.run(move |cx: &mut App| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        clipboard::set_global_capture_filter(CaptureFilter::new());
        clipboard::set_global_store(store.clone());
        clipboard::set_global_vault_controller(vault);
        let (tx, _) = mpsc::channel();
        clipboard::ClipboardChooserWindow::toggle(cx, tx, store, None).unwrap();
    });
    drop(fixtures);
    Ok(())
}
