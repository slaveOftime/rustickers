use anyhow::anyhow;
use gpui::*;
use gpui_component::IconNamed;
use rust_embed::RustEmbed;
use std::borrow::Cow;

pub mod stickers;
pub mod webview;

#[derive(RustEmbed)]
#[folder = "./assets"]
#[include = "icons/**/*.svg"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        Self::get(path)
            .map(|f| Some(f.data))
            .ok_or_else(|| anyhow!("could not find asset at path \"{path}\""))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect())
    }
}

#[allow(dead_code)]
pub enum ExtendedIconName {
    Play,
    Pause,
    Stop,
    Adjustments,
}

impl IconNamed for ExtendedIconName {
    fn path(self) -> SharedString {
        match self {
            ExtendedIconName::Play => "icons/extended/play.svg".into(),
            ExtendedIconName::Pause => "icons/extended/pause.svg".into(),
            ExtendedIconName::Stop => "icons/extended/stop.svg".into(),
            ExtendedIconName::Adjustments => "icons/extended/adjustments.svg".into(),
        }
    }
}
