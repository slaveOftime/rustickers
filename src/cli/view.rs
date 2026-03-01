pub fn run(source: String) -> anyhow::Result<()> {
    match crate::ipc::send_ipc_command("rustickers", &format!("PREVIEW_FILE {source}")) {
        Ok(true) => Ok(()),
        Ok(false) => {
            anyhow::bail!("Rustickers is not running. Please launch it first, then retry.")
        }
        Err(err) => Err(err.into()),
    }
}
