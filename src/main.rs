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
    // Required this for Windows to render the WebView.
    #[cfg(target_os = "windows")]
    unsafe {
        std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "true");
    }

    let mut single_instance = match crate::ipc::SingleInstance::acquire("rustickers") {
        Ok(instance) => Some(instance),
        Err(ipc::AcquireError::AlreadyRunning) => {
            return;
        }
        Err(ipc::AcquireError::Io(err)) => {
            println!("Failed to acquire single instance: {err:#}");
            return;
        }
    };

    let app_paths = AppPaths::new().expect("App paths should initialize");
    let (ipc_events_tx, ipc_events_rx) = mpsc::channel::<IpcEvent>();
    let (sticker_events_tx, sticker_events_rx) = mpsc::channel::<StickerWindowEvent>();

    if let Some(instance) = &mut single_instance {
        instance.start_ipc_server(ipc_events_tx.clone());
    }

    if let Err(err) = crate::hotkey::start_global_hotkey_listener(ipc_events_tx.clone()) {
        println!("Failed to start global hotkey listener: {err:#}");
    }

    let app = Application::new()
        .with_assets(components::Assets)
        .with_http_client(http::ReqwestClient::new());

    let main_window_handle = Arc::new(OnceLock::<AnyWindowHandle>::new());

    app.run(move |cx: &mut App| {
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);

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
            let store: ArcStickerStore = match open_sqlite(app_paths.db_path).await {
                Ok(store) => store,
                Err(err) => {
                    println!("Failed to open store: {err:?}");
                    return;
                }
            };

            match store.get_open_sticker_ids().await {
                Ok(sticker_ids) => {
                    for id in sticker_ids {
                        let store = store.clone();
                        let sticker_events_tx = sticker_events_tx.clone();
                        if let Err(err) =
                            StickerWindow::open_async(cx, sticker_events_tx, store, id).await
                        {
                            println!("Failed to open sticker window for id {id}: {err:?}");
                        }
                    }
                }
                Err(err) => {
                    println!("Failed to get open sticker ids from store: {err:?}");
                }
            }

            let _ = cx.update(move |cx| {
                match MainWindow::open(cx, sticker_events_rx, sticker_events_tx.clone(), store) {
                    Ok(window) => {
                        let _ = main_window_handle_clone.set(window.clone());
                    }
                    Err(err) => {
                        println!("Failed to open main window: {err:?}");
                    }
                }
            });
        })
        .detach();
    });
}
