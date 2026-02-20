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
pub mod hotkey;
pub mod http;
pub mod windows;

pub fn run_native(
    app_paths: AppPaths,
    ipc_events_rx: mpsc::Receiver<IpcEvent>,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    sticker_events_rx: mpsc::Receiver<StickerWindowEvent>,
) {
    let app = Application::new()
        .with_assets(components::Assets)
        .with_http_client(http::ReqwestClient::new());

    let main_window_handle = Arc::new(OnceLock::<AnyWindowHandle>::new());

    app.run(move |cx: &mut App| {
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let theme = cx.global_mut::<Theme>();
        theme.background = rgb(0x151104).into();

        let main_window_handle_clone = main_window_handle.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
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

            tracing::info!("Sticker store opened");

            match store.get_open_sticker_ids().await {
                Ok(sticker_ids) => {
                    tracing::debug!(count = sticker_ids.len(), "Restoring open stickers");
                    for id in sticker_ids {
                        let store = store.clone();
                        let sticker_events_tx = sticker_events_tx.clone();
                        if let Err(err) =
                            StickerWindow::open_async(cx, sticker_events_tx, store, id).await
                        {
                            tracing::warn!(id, error = ?err, "Failed to open sticker window");
                        }
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
