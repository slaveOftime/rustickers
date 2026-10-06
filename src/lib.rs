#[cfg(feature = "cli")]
pub mod cli;
pub mod clipboard;
pub mod ipc;
pub mod model;
#[cfg(feature = "ui")]
pub mod native;
pub mod storage;
pub mod utils;

// `--feature ui` brings in `gpui-kit` (the Kit facade). Existing modules
// still write `use gpui::...` and `use gpui_component::...`; bridge those
// namespaces to the Kit so the dependency swap in milestone 2 is the only
// change required for the API to resolve. Migrating the imports to
// `gpui_kit::...` happens file by file in later milestones.
#[cfg(feature = "ui")]
extern crate gpui_kit as gpui;

#[cfg(feature = "ui")]
pub mod gpui_component {
    pub use gpui_kit::component::*;
}

#[cfg(feature = "ui")]
extern crate gpui_wry;
