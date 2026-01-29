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
pub enum IconName {
    Play,
    Pause,
    Plus,
    Stop,
    Adjustments,
    Close,
    Command,
    DocumentText,
    Bell,
    Minus,
    Minimize,
    Search,
    SortAscending,
    SortDescending,
    Forward,
    ArrowUp,
    ArrowDown,
    Check,
}

impl IconNamed for IconName {
    fn path(self) -> SharedString {
        match self {
            IconName::Play => "icons/play.svg".into(),
            IconName::Pause => "icons/pause.svg".into(),
            IconName::Plus => "icons/plus.svg".into(),
            IconName::Stop => "icons/stop.svg".into(),
            IconName::Adjustments => "icons/adjustments.svg".into(),
            IconName::Close => "icons/close.svg".into(),
            IconName::Command => "icons/command.svg".into(),
            IconName::DocumentText => "icons/document-text.svg".into(),
            IconName::Bell => "icons/bell.svg".into(),
            IconName::Minus => "icons/minus.svg".into(),
            IconName::Minimize => "icons/minimize.svg".into(),
            IconName::Search => "icons/search.svg".into(),
            IconName::SortAscending => "icons/sort-ascending.svg".into(),
            IconName::SortDescending => "icons/sort-descending.svg".into(),
            IconName::Forward => "icons/forward.svg".into(),
            IconName::ArrowUp => "icons/arrow-up.svg".into(),
            IconName::ArrowDown => "icons/arrow-down.svg".into(),
            IconName::Check => "icons/check.svg".into(),
        }
    }
}
