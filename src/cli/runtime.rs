//! Database access and best-effort desktop notification for one invocation.
//!
//! An isolated data directory implies offline operation: IDs from a test database must never
//! control windows belonging to the user's real database. IPC reports transport delivery only,
//! not that the desktop has acted on a request.

use crate::storage::{ArcStickerStore, paths::AppPaths};
use anyhow::Context as _;

pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    futures::executor::block_on(future)
}

pub(crate) struct Runtime {
    paths: AppPaths,
    offline: bool,
}

impl Runtime {
    pub(crate) fn new(paths: AppPaths, offline: bool) -> Self {
        Self { paths, offline }
    }

    pub(crate) fn store(&self) -> anyhow::Result<ArcStickerStore> {
        self.prepare_storage()?;
        block_on(crate::storage::open_sqlite(&self.paths.db_path))
    }

    pub(crate) fn text_repository(&self) -> anyhow::Result<crate::text::TextRepository> {
        self.prepare_storage()?;
        block_on(crate::text::TextRepository::open(&self.paths.db_path))
    }

    fn prepare_storage(&self) -> anyhow::Result<()> {
        if let Some(parent) = self
            .paths
            .db_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).context("create data directory")?;
        }
        Ok(())
    }

    pub(crate) fn send(&self, command: &str) -> Delivery {
        if self.offline {
            return Delivery::Offline;
        }
        match crate::ipc::send_ipc_command("rustickers", command) {
            Ok(true) => Delivery::Sent,
            Ok(false) | Err(_) => Delivery::Unavailable,
        }
    }

    pub(crate) fn open_sticker(&self, id: i64) -> Delivery {
        self.send(&format!("OPEN_STICKER {id}"))
    }

    pub(crate) fn close_sticker(&self, id: i64) -> Delivery {
        self.send(&format!("CLOSE_STICKER {id}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivery {
    Sent,
    Unavailable,
    Offline,
    NotRequested,
}

impl Delivery {
    pub(crate) fn delivered(self) -> bool {
        self == Self::Sent
    }
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Unavailable => "unavailable",
            Self::Offline => "offline",
            Self::NotRequested => "not_requested",
        }
    }
}
