#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod components;
mod hotkey;
mod http;
mod ipc;
mod model;
mod storage;
mod utils;
mod windows;

use gpui::{AnyWindowHandle, App, Application, transparent_black};
use gpui_component::{Theme, ThemeMode};

use std::sync::{Arc, OnceLock, mpsc};
use std::time::Duration;

use storage::{ArcStickerStore, open_sqlite, paths::AppPaths};

use ipc::IpcEvent;
use windows::StickerWindowEvent;
use windows::main::MainWindow;
use windows::sticker::StickerWindow;

fn main() {
    let app_paths = AppPaths::new().expect("App paths should initialize");
    let _ =
        crate::utils::logging::LoggingGuards::init(&app_paths).expect("Logging should initialize");

    tracing::info!(
        app_version = env!("CARGO_PKG_VERSION"),
        debug_build = cfg!(debug_assertions),
        "Starting Rustickers"
    );

    let mut single_instance = match crate::ipc::SingleInstance::acquire("rustickers") {
        Ok(instance) => Some(instance),
        Err(ipc::AcquireError::AlreadyRunning) => {
            tracing::info!("Another instance is already running; exiting");
            return;
        }
        Err(ipc::AcquireError::Io(err)) => {
            tracing::error!(error = %err, "Failed to acquire single instance");
            return;
        }
    };

    tracing::debug!("Single-instance lock acquired");

    let (ipc_events_tx, ipc_events_rx) = mpsc::channel::<IpcEvent>();
    let (sticker_events_tx, sticker_events_rx) = mpsc::channel::<StickerWindowEvent>();

    if let Some(instance) = &mut single_instance {
        instance.start_ipc_server(ipc_events_tx.clone());
    }

    if let Err(err) = crate::hotkey::start_global_hotkey_listener(ipc_events_tx.clone()) {
        tracing::error!(error = %err, "Failed to start global hotkey listener");
    }

    let app = Application::new()
        .with_assets(components::Assets)
        .with_http_client(http::ReqwestClient::new());

    let main_window_handle = Arc::new(OnceLock::<AnyWindowHandle>::new());

    app.run(move |cx: &mut App| {
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);

        // This is needed to make window background fully transparent because gpui-component RootView is is use it as the default background.
        // Next version can be removed
        let theme = cx.global_mut::<Theme>();
        theme.background = transparent_black().alpha(0.0);

        let main_window_handle_clone = main_window_handle.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                while let Ok(event) = ipc_events_rx.try_recv() {
                    match event {
                        IpcEvent::Show => {
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

        let app_paths = app_paths.clone();
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
