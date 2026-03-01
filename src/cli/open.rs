pub fn run(id: i64) -> anyhow::Result<()> {
    match crate::ipc::send_ipc_command("rustickers", &format!("OPEN_STICKER {id}")) {
        Ok(true) => {
            return Ok(());
        }
        Ok(false) => {} // app not running — fall through to direct DB update
        Err(err) => {
            tracing::warn!(error = %err, "IPC send failed; falling back to direct DB update");
        }
    }

    Ok(())
}
