//! `rusticker show` — everything stored about one sticker.

use serde_json::{Value, json};

use crate::model::sticker::StickerDetail;
use crate::storage::paths::AppPaths;

use crate::cli::output::{Format, format_ts};
use crate::cli::runtime::{block_on, open_store};

use super::list::{state_name, type_name};

#[derive(clap::Args, Debug)]
pub struct ShowArgs {
    /// Sticker ID, as listed by `rusticker list`
    pub id: i64,

    /// Print only the stored content, with no surrounding report
    ///
    /// Useful for piping a note somewhere else.
    #[arg(long)]
    pub content_only: bool,
}

/// A sticker's content is a JSON document for some types and free text for others, so parse it
/// when we can and hand back a plain string when we cannot.
///
/// Giving a caller structured content when it exists saves them a second parse and a guess about
/// which sticker types store what.
pub fn content_value(sticker: &StickerDetail) -> Value {
    serde_json::from_str::<Value>(&sticker.content)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::String(sticker.content.clone()))
}

pub fn to_json(sticker: &StickerDetail) -> Value {
    json!({
        "id": sticker.id,
        "title": sticker.title,
        "type": type_name(sticker.sticker_type),
        "state": state_name(sticker.state),
        "color": sticker.color.as_str(),
        "left": sticker.left,
        "top": sticker.top,
        "width": sticker.width,
        "height": sticker.height,
        "top_most": sticker.top_most,
        "created_at": sticker.created_at,
        "updated_at": sticker.updated_at,
        "content": content_value(sticker),
    })
}

pub fn load(app_paths: &AppPaths, id: i64) -> anyhow::Result<StickerDetail> {
    let store = open_store(app_paths)?;
    // The underlying "no rows returned" is noise; the id is the only useful part.
    block_on(store.get_sticker(id)).map_err(|_| anyhow::anyhow!("no sticker with id {id}"))
}

pub fn run(app_paths: &AppPaths, args: ShowArgs, format: Format) -> anyhow::Result<()> {
    let sticker = load(app_paths, args.id)?;

    if args.content_only {
        format.emit(
            json!({ "id": sticker.id, "content": content_value(&sticker) }),
            || {
                println!("{}", sticker.content);
            },
        );
        return Ok(());
    }

    format.emit(to_json(&sticker), || {
        println!("id:         {}", sticker.id);
        println!("title:      {}", sticker.title);
        println!("type:       {}", type_name(sticker.sticker_type));
        println!("state:      {}", state_name(sticker.state));
        println!("color:      {}", sticker.color.as_str());
        println!("position:   {},{}", sticker.left, sticker.top);
        println!("size:       {}x{}", sticker.width, sticker.height);
        println!("top_most:   {}", sticker.top_most);
        println!("created_at: {}", format_ts(sticker.created_at));
        println!("updated_at: {}", format_ts(sticker.updated_at));
        println!("content:");
        // Pretty-print structured content; a wall of one-line JSON is unreadable.
        match content_value(&sticker) {
            Value::String(text) => println!("{text}"),
            structured => println!(
                "{}",
                serde_json::to_string_pretty(&structured).unwrap_or(sticker.content.clone())
            ),
        }
    });

    Ok(())
}
