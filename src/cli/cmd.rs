use std::io::Write as _;

use crate::model::content::{CommandContent, CommandResult, Scheduler};
use crate::model::sticker::{StickerColor, StickerDetail, StickerState, StickerType};
use crate::storage::paths::AppPaths;

use super::console::{block_on, console_writer, signal_open, truncate};

pub fn run(
    app_paths: &AppPaths,
    command: String,
    cron: Option<String>,
    run_immediately: bool,
    env: Vec<String>,
    dir: Option<String>,
) -> anyhow::Result<()> {
    let store = block_on(crate::storage::open_sqlite(&app_paths.db_path))?;

    let content = CommandContent {
        command: command.clone(),
        environments: env.join("\n"),
        working_dir: dir.unwrap_or_default(),
        scheduler: cron.map(Scheduler::Cron),
        run_immediately,
        result: CommandResult::Text(None),
        stream_result: false,
        padding: None,
        started_at: None,
    };

    let content_json = serde_json::to_string(&content)?;
    let title = truncate(&command, 30).to_owned();

    let id = block_on(store.insert_sticker(StickerDetail {
        id: 0,
        title,
        state: StickerState::Open,
        left: 100,
        top: 100,
        width: 400,
        height: 300,
        top_most: false,
        color: StickerColor::Gray,
        sticker_type: StickerType::Command,
        content: content_json,
        created_at: 0,
        updated_at: 0,
        display_id: None,
    }))?;

    let mut out = console_writer();
    writeln!(out, "Created command sticker (id={id})")?;
    signal_open(id, &mut *out);
    Ok(())
}
