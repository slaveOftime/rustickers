use std::io::Write as _;

use crate::storage::paths::AppPaths;

use super::console::{block_on, console_writer, truncate};

pub fn run(app_paths: &AppPaths) -> anyhow::Result<()> {
    let store = block_on(crate::storage::open_sqlite(&app_paths.db_path))?;
    let ids = block_on(store.get_open_sticker_ids())?;

    let mut out = console_writer();

    if ids.is_empty() {
        writeln!(out, "No open stickers.")?;
        return Ok(());
    }

    writeln!(
        out,
        "{:<6}  {:<30}  {:<10}  {:<12}  {}",
        "ID", "TITLE", "TYPE", "POSITION", "SIZE"
    )?;
    writeln!(out, "{}", "-".repeat(70))?;

    for id in ids {
        match block_on(store.get_sticker(id)) {
            Ok(d) => {
                let title = truncate(&d.title, 30);
                let type_str = format!("{:?}", d.sticker_type);
                let pos = format!("{},{}", d.left, d.top);
                let size = format!("{}x{}", d.width, d.height);
                writeln!(
                    out,
                    "{:<6}  {:<30}  {:<10}  {:<12}  {}",
                    id, title, type_str, pos, size
                )?;
            }
            Err(err) => {
                writeln!(out, "{:<6}  [error: {}]", id, err)?;
            }
        }
    }

    Ok(())
}
