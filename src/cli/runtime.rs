//! The two things every command needs: the sticker database, and a way to talk to a running app.

use crate::storage::ArcStickerStore;
use crate::storage::paths::AppPaths;

/// The IPC identity of the desktop app. The CLI is a different binary but the same application.
const APP_ID: &str = "rustickers";

pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

pub fn open_store(app_paths: &AppPaths) -> anyhow::Result<ArcStickerStore> {
    block_on(crate::storage::open_sqlite(&app_paths.db_path))
}

/// Whether a message reached the desktop app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The app is running and acted on the message.
    Delivered,
    /// The app is not running. The database is still the source of truth, so whatever we wrote
    /// takes effect the next time it launches.
    AppNotRunning,
}

impl Delivery {
    pub fn delivered(self) -> bool {
        self == Self::Delivered
    }
}

/// Send one IPC verb to the desktop app.
///
/// A transport error is reported as `AppNotRunning` rather than propagated: every caller has
/// already persisted its change to the database, so a missing app is a delivery detail, not a
/// failure of the command.
pub fn send(command: &str) -> Delivery {
    match crate::ipc::send_ipc_command(APP_ID, command) {
        Ok(true) => Delivery::Delivered,
        Ok(false) => Delivery::AppNotRunning,
        Err(err) => {
            tracing::warn!(error = %err, command, "IPC send failed");
            Delivery::AppNotRunning
        }
    }
}

pub fn open_sticker(id: i64) -> Delivery {
    send(&format!("OPEN_STICKER {id}"))
}

pub fn close_sticker(id: i64) -> Delivery {
    send(&format!("CLOSE_STICKER {id}"))
}
