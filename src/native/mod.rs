use std::{
    sync::{Arc, OnceLock, mpsc},
    time::Duration,
};

use crate::gpui_component::{Theme, ThemeMode};
use gpui::{AnyWindowHandle, App};

use crate::{
    ipc::IpcEvent,
    native::windows::{StickerWindowEvent, main::MainWindow, sticker::StickerWindow},
    storage::{ArcStickerStore, open_sqlite, paths::AppPaths},
    text::TextRepository,
};

pub mod clipboard;
pub mod components;
pub mod file_manager;
pub mod hotkey;
pub mod http;
pub mod power_guard;
pub mod selection;
pub mod theme;
pub mod windows;

pub fn run_native(
    app_paths: AppPaths,
    ipc_events_rx: mpsc::Receiver<IpcEvent>,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    sticker_events_rx: mpsc::Receiver<StickerWindowEvent>,
) {
    // Per-pixel window alpha requires GPUI's DirectComposition swap chain.
    // The legacy HWND swap chain uses DXGI_ALPHA_MODE_IGNORE. Leave the
    // framework default enabled, while honoring explicit operator overrides.
    // Sticker Root and editor backgrounds must also stay transparent; enabling
    // composition alone cannot reveal the desktop through opaque UI layers.
    #[cfg(target_os = "windows")]
    {
        if let Some(value) = std::env::var_os("GPUI_DISABLE_DIRECT_COMPOSITION") {
            tracing::info!(
                value = %value.to_string_lossy(),
                "GPUI_DISABLE_DIRECT_COMPOSITION is set externally; honoring it"
            );
            if value == "true" || value == "1" {
                tracing::warn!(
                    "DirectComposition is disabled; per-pixel window transparency is unavailable"
                );
            }
        }
    }

    let app = gpui::application()
        .with_assets(components::Assets)
        .with_http_client(http::ReqwestClient::new());

    // Must run after the platform (and its hidden message window) exists,
    // but before the event loop starts dispatching messages.
    power_guard::install();

    let main_window_handle = Arc::new(OnceLock::<AnyWindowHandle>::new());
    let store_handle = Arc::new(OnceLock::<ArcStickerStore>::new());
    let clipboard_handle_for_open: Arc<OnceLock<Arc<TextRepository>>> = Arc::new(OnceLock::new());
    let clipboard_handle_for_ipc = clipboard_handle_for_open.clone();
    let clipboard_filter_for_open: Arc<
        OnceLock<std::sync::Arc<crate::native::clipboard::CaptureFilter>>,
    > = Arc::new(OnceLock::new());
    let clipboard_filter_for_ipc = clipboard_filter_for_open.clone();

    app.run(move |cx: &mut App| {
        cx.on_app_quit(|_| async { crate::native::windows::text::clear_owned_secret(); }).detach();
        crate::gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);

        let main_window_handle_clone = main_window_handle.clone();
        let store_handle_clone = store_handle.clone();
        let sticker_events_tx_for_ipc = sticker_events_tx.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                while let Ok(event) = ipc_events_rx.try_recv() {
                    match event {
                        crate::ipc::IpcEvent::Show => {
                            if let Some(handle) = main_window_handle_clone.get() {
                                let _ = handle.update(cx, |_, window, _| {
                                    window.activate_window();
                                });
                            }
                        }
                        crate::ipc::IpcEvent::ToggleFilePreview => {
                            if let Some(store) = store_handle_clone.get() {
                                cx.update(|cx| {
                                    if let Err(err) = StickerWindow::open_file_preview(
                                        cx,
                                        sticker_events_tx_for_ipc.clone(),
                                        store.clone(),
                                    ) {
                                        tracing::warn!(error = ?err, "Failed to toggle file sticker preview");
                                    } else {
                                        cx.refresh_windows();
                                    }
                                });
                            }
                        }
                        crate::ipc::IpcEvent::DismissEscapeTarget => {
                            cx.update(|cx| {
                                crate::native::windows::close_active_escape_target(cx);
                            });
                        }
                        crate::ipc::IpcEvent::OpenSticker(id) => {
                            if let Some(store) = store_handle_clone.get() {
                                let store = store.clone();
                                let tx = sticker_events_tx_for_ipc.clone();
                                if let Err(err) =
                                    StickerWindow::open_async(cx, tx, store, id).await
                                {
                                    tracing::warn!(id, error = ?err, "Failed to open sticker from IPC");
                                }
                            }
                        }
                        crate::ipc::IpcEvent::PreviewFile(request) => {
                            if let Some(store) = store_handle_clone.get() {
                                cx.update(|cx| {
                                    if let Err(err) = StickerWindow::open_file_preview_with_sources(
                                        cx,
                                        sticker_events_tx_for_ipc.clone(),
                                        store.clone(),
                                        vec![request.source],
                                        crate::native::windows::sticker::PreviewOptions {
                                            width: request.width,
                                            height: request.height,
                                            color: request.color,
                                            flash: request.flash,
                                        },
                                    ) {
                                        tracing::warn!(error = ?err, "Failed to open file preview from IPC");
                                    }
                                });
                            }
                        }
                        crate::ipc::IpcEvent::CloseSticker(id) => {
                            if let Some(store) = store_handle_clone.get() {
                                let store = store.clone();
                                if let Err(err) =
                                    store.update_sticker_state(id, crate::model::sticker::StickerState::Close).await
                                {
                                    tracing::error!(id, error = %err, "Error closing sticker from IPC");
                                }
                                cx.update(|cx| {
                                    StickerWindow::try_close(id, cx);
                                });
                            }
                        }
                        crate::ipc::IpcEvent::TriggerSelectionToCommand => {
                            if let Some(store) = store_handle_clone.get() {
                                let store = store.clone();
                                let tx = sticker_events_tx_for_ipc.clone();
                                let source = crate::native::selection::foreground_window();
                                let filter = clipboard_filter_for_ipc.get().cloned()
                                    .unwrap_or_else(crate::native::clipboard::CaptureFilter::new);
                                // Leave the source focused and the UI message pump running
                                // while shortcut modifiers release and Copy is processed.
                                let selection = cx.background_executor().spawn(async move {
                                    crate::native::selection::capture_selection_from_window(&filter, source)
                                }).await;
                                let selection = match selection {
                                    Ok(selection) => selection,
                                    Err(err) => {
                                        tracing::warn!(error = ?err, "Failed to capture text selection");
                                        None
                                    }
                                };
                                match selection {
                                    Some(selection) => {
                                        cx.spawn(async move |cx| {
                                            match open_selection_command(cx, tx, store, &selection).await {
                                                Ok(count) => {
                                                    tracing::info!(eligible_count = count, "Opened selection command chooser");
                                                }
                                                Err(err) => {
                                                    tracing::warn!(error = ?err, "Failed to open selection command chooser");
                                                }
                                            }
                                        }).detach();
                                    }
                                    // No direct selection: ask the user to type the text instead
                                    // of falling back to whatever is in the clipboard.
                                    None => {
                                        tracing::info!("No text selection captured, asking for manual input");
                                        cx.update(|cx| {
                                            if let Err(err) = crate::native::windows::selection::SelectionPopup::open_for_input(
                                                cx,
                                                tx,
                                                store,
                                            ) {
                                                tracing::warn!(error = ?err, "Failed to open selection input popup");
                                            }
                                        });
                                    }
                                }
                            }
                        }
                        crate::ipc::IpcEvent::ToggleClipboardChooser { focused_window } => {
                            let store = clipboard_handle_for_ipc.get().cloned();
                            let filter = clipboard_filter_for_ipc.get().cloned();
                            if let (Some(store), Some(_filter)) = (store, filter) {
                                cx.update(|cx| {
                                    if let Err(err) = crate::native::windows::text::TextPickerWindow::toggle(
                                        cx,
                                        sticker_events_tx_for_ipc.clone(),
                                        store,
                                        focused_window,
                                    ) {
                                        tracing::warn!(error = ?err, "Failed to toggle clipboard chooser");
                                    }
                                });
                            } else {
                                tracing::warn!(
                                    "Clipboard chooser requested but the clipboard backend has not been initialised"
                                );
                            }
                        }
                    }
                }
            }
        })
        .detach();

        let main_window_handle_clone = main_window_handle.clone();
        cx.spawn(async move |cx| {
            let sticker_db = app_paths.db_path.clone();
            tracing::info!(db_path = %sticker_db.display(), "Opening sticker store");
            let store: ArcStickerStore = match open_sqlite(&sticker_db).await {
                Ok(store) => store,
                Err(err) => {
                    tracing::error!(error = ?err, "Failed to open store");
                    return;
                }
            };

            let _ = store_handle.set(store.clone());

            tracing::info!("Sticker store opened");

            // Clipboard failure must not prevent the main window or stickers from opening.
            match TextRepository::open(&sticker_db).await {
                Ok(backend) => {
                    let clipboard_store = Arc::new(backend);
                    let filter = crate::native::clipboard::CaptureFilter::new();
                    let _ = clipboard_handle_for_open.set(clipboard_store.clone());
                    let _ = clipboard_filter_for_open.set(filter.clone());
                    let (watcher, _stop) = crate::native::clipboard::watcher::WatcherHandle::new(Duration::from_millis(250));
                    watcher.spawn(filter.clone(), clipboard_store.clone());
                    crate::native::windows::text::set_global_store(clipboard_store, store.clone());
                    cx.update(move |_cx| {
                        crate::native::windows::text::set_global_capture_filter(filter);
                    });
                }
                Err(_) => tracing::error!("Clipboard backend unavailable; continuing sticker startup"),
            }

            // Armed cron stickers have to keep ticking whether or not their window is open, so the
            // scheduler starts before any window is restored.
            let scheduler_store = store.clone();
            cx.update(move |cx| {
                components::stickers::command::background::start(cx, scheduler_store);
            });

            match store.get_open_sticker_ids().await {
                Ok(sticker_ids) => {
                    tracing::debug!(count = sticker_ids.len(), "Restoring open stickers");
                    for id in sticker_ids {
                        let store = store.clone();
                        let sticker_events_tx = sticker_events_tx.clone();
                        cx.spawn(async move |cx| {
                            if let Err(err) =
                                StickerWindow::open_async(cx, sticker_events_tx, store, id).await
                            {
                                tracing::warn!(id, error = ?err, "Failed to open sticker window");
                            }
                        }).detach();
                    }
                }
                Err(err) => {
                    tracing::error!(error = ?err, "Failed to get open sticker ids from store");
                }
            }

            cx.update(move |cx| {
                match MainWindow::open(cx, sticker_events_rx, sticker_events_tx.clone(), store) {
                    Ok(window) => {
                        let _ = main_window_handle_clone.set(window);
                        tracing::info!("Main window opened");
                    }
                    Err(err) => {
                        tracing::error!(error = ?err, "Failed to open main window");
                    }
                }
            });
        })
        .detach();
    });
}

/// What to do with the command stickers eligible for a selection.
pub(crate) enum SelectionCommandTarget {
    /// Only one sticker is eligible, it can be run right away.
    Single(Box<crate::model::sticker::StickerDetail>),
    /// Several stickers are eligible, the user has to pick one.
    Choose(Vec<crate::model::sticker::StickerDetail>),
}

/// Load the command stickers that accept a selection, in LRU order.
pub(crate) async fn resolve_selection_command(
    store: &ArcStickerStore,
) -> anyhow::Result<SelectionCommandTarget> {
    let mut stickers = store.get_accept_selection_stickers().await?;
    if stickers.is_empty() {
        return Err(anyhow::anyhow!(
            "No command stickers with accept_selection enabled"
        ));
    }

    Ok(if stickers.len() == 1 {
        SelectionCommandTarget::Single(Box::new(stickers.remove(0)))
    } else {
        SelectionCommandTarget::Choose(stickers)
    })
}

/// Open the only eligible selection command, or show a chooser when several match.
async fn open_selection_command(
    cx: &mut gpui::AsyncApp,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    store: ArcStickerStore,
    selection: &str,
) -> anyhow::Result<usize> {
    match resolve_selection_command(&store).await? {
        SelectionCommandTarget::Single(sticker) => {
            tracing::debug!(
                sticker_id = sticker.id,
                selection_len = selection.len(),
                "Opening the only eligible selection command"
            );
            if let Err(err) = store
                .touch_selection_lru(sticker.id, crate::utils::time::now_unix_millis())
                .await
            {
                tracing::warn!(sticker_id = sticker.id, error = ?err, "Failed to update selection command LRU");
            }
            cx.update(|cx| {
                StickerWindow::open_with_selection(
                    cx,
                    sticker_events_tx,
                    store,
                    *sticker,
                    selection.to_owned(),
                )
            })?;
            Ok(1)
        }
        SelectionCommandTarget::Choose(stickers) => {
            let count = stickers.len();
            tracing::debug!(
                count,
                selection_len = selection.len(),
                "Opening selection command chooser"
            );
            cx.update(|cx| {
                crate::native::windows::selection::SelectionPopup::open(
                    cx,
                    sticker_events_tx,
                    store,
                    stickers,
                    selection.to_owned(),
                )
            })?;
            Ok(count)
        }
    }
}
