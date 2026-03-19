use std::path::Path;

pub fn run(source: String) -> anyhow::Result<()> {
    // If the source is not a URL and is a relative file path, resolve it to absolute
    let resolved_source = if !crate::utils::url::is_url(&source) {
        let path = Path::new(&source);
        if path.is_relative() {
            std::env::current_dir()?
                .join(path)
                .to_string_lossy()
                .to_string()
        } else {
            source
        }
    } else {
        source
    };

    match crate::ipc::send_ipc_command("rustickers", &format!("PREVIEW_FILE {resolved_source}")) {
        Ok(true) => Ok(()),
        Ok(false) => {
            anyhow::bail!("Rustickers is not running. Please launch it first, then retry.")
        }
        Err(err) => Err(err.into()),
    }
}
