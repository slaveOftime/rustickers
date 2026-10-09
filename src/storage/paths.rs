use anyhow::Context as _;
use directories::ProjectDirs;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub db_path: PathBuf,
}

impl AppPaths {
    pub fn default_location() -> anyhow::Result<Self> {
        let project_dirs =
            ProjectDirs::from("", "", "rustickers").context("resolve AppData project directory")?;

        let data_dir = project_dirs.data_local_dir().to_path_buf();
        let db_path = data_dir.join("stickers.db");

        Ok(Self { db_path })
    }

    pub fn new() -> anyhow::Result<Self> {
        let paths = Self::default_location()?;
        if let Some(data_dir) = paths.db_path.parent() {
            fs::create_dir_all(data_dir).context("create AppData data dir")?;
        }
        Ok(paths)
    }

    pub fn log_dir(&self) -> PathBuf {
        // Keep logs in a dedicated folder alongside the database.
        // If the db path changes in the future, logs follow automatically.
        self.db_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("logs")
    }
}
