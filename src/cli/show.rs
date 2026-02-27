use std::io::Write as _;

use anyhow::Context as _;

use crate::storage::paths::AppPaths;

use super::console::{block_on, console_writer, format_ts};

pub fn run(app_paths: &AppPaths, id: i64) -> anyhow::Result<()> {
    let store = block_on(crate::storage::open_sqlite(&app_paths.db_path))?;
    let d = block_on(store.get_sticker(id)).with_context(|| format!("sticker {id} not found"))?;

    let mut out = console_writer();
    writeln!(out, "id:         {}", d.id)?;
    writeln!(out, "title:      {}", d.title)?;
    writeln!(out, "type:       {:?}", d.sticker_type)?;
    writeln!(out, "state:      {:?}", d.state)?;
    writeln!(out, "color:      {}", d.color.as_str())?;
    writeln!(out, "position:   left={}, top={}", d.left, d.top)?;
    writeln!(out, "size:       {}x{}", d.width, d.height)?;
    writeln!(out, "top_most:   {}", d.top_most)?;
    writeln!(out, "created_at: {}", format_ts(d.created_at))?;
    writeln!(out, "updated_at: {}", format_ts(d.updated_at))?;
    writeln!(out, "content:")?;
    writeln!(out, "{}", d.content)?;

    Ok(())
}
