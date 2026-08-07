//! `rusticker list` — find stickers.

use clap::ValueEnum;
use serde_json::json;

use crate::model::sticker::{StickerListItem, StickerState, StickerType};
use crate::storage::paths::AppPaths;

use crate::cli::output::{Format, one_line, table};
use crate::cli::runtime::{block_on, open_store};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StateFilter {
    Open,
    Close,
    All,
}

impl StateFilter {
    fn to_state(self) -> Option<StickerState> {
        match self {
            Self::Open => Some(StickerState::Open),
            Self::Close => Some(StickerState::Close),
            Self::All => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TypeFilter {
    Markdown,
    Timer,
    Command,
    Paint,
    File,
}

impl TypeFilter {
    fn matches(self, sticker_type: StickerType) -> bool {
        matches!(
            (self, sticker_type),
            (Self::Markdown, StickerType::Markdown)
                | (Self::Timer, StickerType::Timer)
                | (Self::Command, StickerType::Command)
                | (Self::Paint, StickerType::Paint)
                | (Self::File, StickerType::File)
        )
    }
}

#[derive(clap::Args, Debug)]
pub struct ListArgs {
    /// Which stickers to include
    #[arg(long, value_enum, default_value = "open")]
    pub state: StateFilter,

    /// Only show stickers of this kind
    #[arg(long = "type", value_enum, value_name = "KIND")]
    pub sticker_type: Option<TypeFilter>,

    /// Match against the title and the content
    #[arg(long, short = 's', value_name = "TEXT")]
    pub search: Option<String>,

    /// Show at most this many stickers
    #[arg(long, short = 'n', value_name = "COUNT")]
    pub limit: Option<usize>,
}

pub fn type_name(sticker_type: StickerType) -> &'static str {
    match sticker_type {
        StickerType::Markdown => "markdown",
        StickerType::Timer => "timer",
        StickerType::Command => "command",
        StickerType::Paint => "paint",
        StickerType::File => "file",
    }
}

pub fn state_name(state: StickerState) -> &'static str {
    match state {
        StickerState::Open => "open",
        StickerState::Close => "close",
    }
}

fn to_json(sticker: &StickerListItem) -> serde_json::Value {
    json!({
        "id": sticker.id,
        "title": sticker.title,
        "type": type_name(sticker.sticker_type),
        "state": state_name(sticker.state),
        "left": sticker.left,
        "top": sticker.top,
        "width": sticker.width,
        "height": sticker.height,
    })
}

pub fn run(app_paths: &AppPaths, args: ListArgs, format: Format) -> anyhow::Result<()> {
    let store = open_store(app_paths)?;
    let mut stickers = block_on(store.list_stickers(args.state.to_state(), args.search.clone()))?;

    if let Some(filter) = args.sticker_type {
        stickers.retain(|sticker| filter.matches(sticker.sticker_type));
    }
    let total = stickers.len();
    if let Some(limit) = args.limit {
        stickers.truncate(limit);
    }

    format.emit(
        json!({
            "stickers": stickers.iter().map(to_json).collect::<Vec<_>>(),
            "count": stickers.len(),
            "total": total,
        }),
        || {
            if stickers.is_empty() {
                if args.state == StateFilter::Open && args.search.is_none() {
                    println!(
                        "No open stickers. Try `rusticker list --state all`, or `rusticker skill \
                         list` for ready-made ones."
                    );
                } else {
                    println!("No stickers match.");
                }
                return;
            }

            let rows: Vec<Vec<String>> = stickers
                .iter()
                .map(|sticker| {
                    vec![
                        sticker.id.to_string(),
                        type_name(sticker.sticker_type).to_owned(),
                        state_name(sticker.state).to_owned(),
                        format!("{},{}", sticker.left, sticker.top),
                        format!("{}x{}", sticker.width, sticker.height),
                        one_line(&sticker.title, 48),
                    ]
                })
                .collect();

            table(&["ID", "TYPE", "STATE", "AT", "SIZE", "TITLE"], &rows);

            if stickers.len() < total {
                println!("\nShowing {} of {total}.", stickers.len());
            }
        },
    );

    Ok(())
}
