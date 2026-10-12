//! Desktop text workflows: picker, public titles, and per-action authorization.
pub(crate) mod password;
mod picker;
mod platform;
pub(crate) mod title;

pub use picker::{
    TextPickerWindow, dispatch_escape, open_from_main, set_global_capture_filter, set_global_store,
};
pub(crate) use picker::{clear_owned_secret, text_repository};
