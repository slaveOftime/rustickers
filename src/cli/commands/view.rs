//! `rusticker view` — preview a file, folder or URL.
//!
//! Unlike the other create commands this one does not write to the database. The preview is a
//! live request to the running app, which decides how to render the source, so there is nothing
//! useful to do when the app is not running.

use anyhow::bail;
use serde_json::json;
use std::path::Path;

use crate::ipc::PreviewFileRequest;
use crate::model::sticker::StickerColor;

use crate::cli::output::Format;
use crate::cli::runtime;

#[derive(clap::Args, Debug)]
pub struct ViewArgs {
    /// File path, folder path or URL to display
    #[arg(value_name = "PATH_OR_URL")]
    pub source: String,

    /// Sticker width in pixels; auto-detected from the source by default
    #[arg(long, value_name = "PX")]
    pub width: Option<i32>,

    /// Sticker height in pixels; auto-detected from the source by default
    #[arg(long, value_name = "PX")]
    pub height: Option<i32>,

    /// Sticker color: yellow, green, blue, pink or gray
    #[arg(long, value_parser = crate::cli::parse_color)]
    pub color: Option<StickerColor>,

    /// Flash the background in the sticker color
    #[arg(long)]
    pub flash: bool,
}

/// The app has its own working directory, so a relative path has to be resolved here or it would
/// be looked up somewhere the user never meant.
fn resolve(source: &str) -> anyhow::Result<String> {
    if crate::utils::url::is_url(source) {
        return Ok(source.to_owned());
    }

    let path = Path::new(source);
    if path.is_relative() {
        Ok(std::env::current_dir()?
            .join(path)
            .to_string_lossy()
            .into_owned())
    } else {
        Ok(source.to_owned())
    }
}

pub fn run(args: ViewArgs, format: Format) -> anyhow::Result<()> {
    let source = resolve(&args.source)?;

    let request = PreviewFileRequest {
        source: source.clone(),
        width: args.width,
        height: args.height,
        color: args.color,
        flash: args.flash,
    };

    let delivery = runtime::send(&format!(
        "PREVIEW_FILE {}",
        serde_json::to_string(&request)?
    ));
    if !delivery.delivered() {
        bail!("Rustickers is not running; `view` needs the app to render the preview");
    }

    format.emit(json!({ "source": source, "app_running": true }), || {
        println!("Previewing {source}")
    });

    Ok(())
}
