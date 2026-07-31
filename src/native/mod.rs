use std::{
    sync::{Arc, OnceLock, mpsc},
    time::Duration,
};

use gpui::{AnyWindowHandle, App, Application, rgb};
use gpui_component::{Theme, ThemeMode};

use crate::{
    ipc::IpcEvent,
    native::windows::{StickerWindowEvent, main::MainWindow, sticker::StickerWindow},
    storage::{ArcStickerStore, open_sqlite, paths::AppPaths},
};

pub mod components;
pub mod file_manager;
pub mod hotkey;
pub mod http;
pub mod selection;
pub mod windows;

pub fn run_native(
    app_paths: AppPaths,
    ipc_events_rx: mpsc::Receiver<IpcEvent>,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    sticker_events_rx: mpsc::Receiver<StickerWindowEvent>,
) {
    let app = Application::with_platform(gpui_platform::current_platform(false))
        .with_assets(components::Assets)
        .with_http_client(http::ReqwestClient::new());

    let main_window_handle = Arc::new(OnceLock::<AnyWindowHandle>::new());
    let store_handle = Arc::new(OnceLock::<ArcStickerStore>::new());

    app.run(move |cx: &mut App| {
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let theme = cx.global_mut::<Theme>();
        theme.background = rgb(0x151104).into();

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
                                        request.width,
                                        request.height,
                                        request.color,
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
                                let _ = cx.update(|cx| {
                                    StickerWindow::try_close(id, cx);
                                });
                            }
                        }
                        crate::ipc::IpcEvent::TriggerSelectionToCommand => {
                            if let Some(store) = store_handle_clone.get() {
                                match crate::native::selection::capture_selection() {
                                    Ok(selection) => {
                                        let store = store.clone();
                                        let tx = sticker_events_tx_for_ipc.clone();
                                        cx.spawn(async move |cx| {
                                            match cascade_open_selection(cx, tx, store, &selection).await {
                                                Ok(count) => {
                                                    tracing::info!(eligible_count = count, "Opened selection command sticker carousel");
                                                }
                                                Err(err) => {
                                                    tracing::warn!(error = ?err, "Failed to cascade-open selection stickers");
                                                }
                                            }
                                        }).detach();
                                    }
                                    Err(err) => {
                                        tracing::warn!(error = ?err, "Failed to capture clipboard selection");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })
        .detach();

        let main_window_handle_clone = main_window_handle.clone();
        cx.spawn(async move |cx| {
            tracing::info!(db_path = %app_paths.db_path.display(), "Opening sticker store");
            let store: ArcStickerStore = match open_sqlite(app_paths.db_path).await {
                Ok(store) => store,
                Err(err) => {
                    tracing::error!(error = ?err, "Failed to open store");
                    return;
                }
            };

            let _ = store_handle.set(store.clone());

            tracing::info!("Sticker store opened");

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

            let _ = cx.update(move |cx| {
                match MainWindow::open(cx, sticker_events_rx, sticker_events_tx.clone(), store) {
                    Ok(window) => {
                        let _ = main_window_handle_clone.set(window.clone());
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

/// Open command stickers with accept_selection=true in an LRU-ordered carousel.
async fn cascade_open_selection(
    cx: &mut gpui::AsyncApp,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    store: ArcStickerStore,
    selection: &str,
) -> anyhow::Result<usize> {
    let stickers = store.get_accept_selection_stickers().await?;
    if stickers.is_empty() {
        return Err(anyhow::anyhow!(
            "No command stickers with accept_selection enabled"
        ));
    }

    tracing::debug!(
        count = stickers.len(),
        selection_len = selection.len(),
        "Opening selection command sticker carousel"
    );

    store
        .touch_selection_lru(stickers[0].id, crate::utils::time::now_unix_millis())
        .await?;

    let count = stickers.len();
    cx.update(|cx| {
        StickerWindow::open_selection_carousel(
            cx,
            sticker_events_tx,
            store,
            stickers,
            selection.to_owned(),
        )
    })?;
    Ok(count)
}
