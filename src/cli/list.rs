use super::console::{block_on, truncate};
use crate::storage::paths::AppPaths;

pub fn run(app_paths: &AppPaths) -> anyhow::Result<()> {
    let store = block_on(crate::storage::open_sqlite(&app_paths.db_path))?;
    let ids = block_on(store.get_open_sticker_ids())?;

    if ids.is_empty() {
        println!("No open stickers.");
        return Ok(());
    }

    println!(
        "{:<6}  {:<30}  {:<10}  {:<12}  {}",
        "ID", "TITLE", "TYPE", "POSITION", "SIZE"
    );
    println!("{}", "-".repeat(70));

    for id in ids {
        match block_on(store.get_sticker(id)) {
            Ok(d) => {
                let title = truncate(&d.title, 30);
                let type_str = format!("{:?}", d.sticker_type);
                let pos = format!("{},{}", d.left, d.top);
                let size = format!("{}x{}", d.width, d.height);
                println!(
                    "{:<6}  {:<30}  {:<10}  {:<12}  {}",
                    id, title, type_str, pos, size
                );
            }
            Err(err) => {
                println!("{:<6}  [error: {}]", id, err);
            }
        }
    }

    Ok(())
}
