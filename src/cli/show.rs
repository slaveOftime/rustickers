use super::console::{block_on, format_ts};
use crate::storage::paths::AppPaths;
use anyhow::Context as _;

pub fn run(app_paths: &AppPaths, id: i64) -> anyhow::Result<()> {
    let store = block_on(crate::storage::open_sqlite(&app_paths.db_path))?;
    let d = block_on(store.get_sticker(id)).with_context(|| format!("sticker {id} not found"))?;

    println!("id:         {}", d.id);
    println!("title:      {}", d.title);
    println!("type:       {:?}", d.sticker_type);
    println!("state:      {:?}", d.state);
    println!("color:      {}", d.color.as_str());
    println!("position:   left={}, top={}", d.left, d.top);
    println!("size:       {}x{}", d.width, d.height);
    println!("top_most:   {}", d.top_most);
    println!("created_at: {}", format_ts(d.created_at));
    println!("updated_at: {}", format_ts(d.updated_at));
    println!("content:");
    println!("{}", d.content);

    Ok(())
}
