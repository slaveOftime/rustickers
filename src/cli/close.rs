use super::console::block_on;
use crate::storage::paths::AppPaths;
use anyhow::Context as _;

pub fn run(app_paths: &AppPaths, id: i64) -> anyhow::Result<()> {
    // Try the running instance first so the window is also removed.
    match crate::ipc::send_ipc_command("rustickers", &format!("CLOSE_STICKER {id}")) {
        Ok(true) => {
            return Ok(());
        }
        Ok(false) => {} // app not running — fall through to direct DB update
        Err(err) => {
            tracing::warn!(error = %err, "IPC send failed; falling back to direct DB update");
        }
    }

    // App is not running: update DB state directly.
    let store = block_on(crate::storage::open_sqlite(&app_paths.db_path))?;
    block_on(store.update_sticker_state(id, crate::model::sticker::StickerState::Close))
        .with_context(|| format!("failed to close sticker {id}"))?;

    println!("Closed sticker {id} (app was not running; state updated in DB).");

    Ok(())
}
