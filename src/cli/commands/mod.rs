//! One module per subcommand.
//!
//! Each module owns its own clap arguments as well as its behaviour, so adding a flag means
//! touching one file rather than threading a new parameter through the dispatcher.

pub mod cmd;
pub mod delete;
pub mod list;
pub mod markdown;
pub mod result;
pub mod show;
pub mod skill;
pub mod view;
pub mod window;
