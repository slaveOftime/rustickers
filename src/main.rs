#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod ipc;
mod model;
mod native;
mod storage;
mod utils;

use ipc::IpcEvent;
use native::windows::StickerWindowEvent;
use std::sync::mpsc;
use storage::paths::AppPaths;

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

    if let Err(err) = crate::native::hotkey::start_global_hotkey_listener(ipc_events_tx.clone()) {
        tracing::error!(error = %err, "Failed to start global hotkey listener");
    }

    crate::native::run_native(
        app_paths,
        ipc_events_rx,
        sticker_events_tx,
        sticker_events_rx,
    );
}
