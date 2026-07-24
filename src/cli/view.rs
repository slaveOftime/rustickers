use std::path::Path;

use crate::ipc::PreviewFileRequest;
use crate::model::sticker::StickerColor;

pub fn run(
    source: String,
    width: Option<i32>,
    height: Option<i32>,
    color: Option<StickerColor>,
) -> anyhow::Result<()> {
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

    let request = PreviewFileRequest {
        source: resolved_source,
        width,
        height,
        color,
    };
    let json = serde_json::to_string(&request)?;

    match crate::ipc::send_ipc_command("rustickers", &format!("PREVIEW_FILE {json}")) {
        Ok(true) => Ok(()),
        Ok(false) => {
            anyhow::bail!("Rustickers is not running. Please launch it first, then retry.")
        }
        Err(err) => Err(err.into()),
    }
}
