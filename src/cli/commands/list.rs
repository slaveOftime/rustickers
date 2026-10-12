//! `rst list` — find stickers.
//!
//! Stickers only. Text entries and clipboard captures live under `rst text list`, which reads a
//! different store; the two listings never share flags so neither can silently change meaning.

use clap::ValueEnum;
use serde_json::json;

use crate::model::sticker::{StickerListItem, StickerState, StickerType};

use crate::cli::output::{Format, one_line, table};
use crate::cli::runtime::{Runtime, block_on};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StateFilter {
    All,
    Open,
    Closed,
}

impl StateFilter {
    /// `None` means "do not filter"; `all` is the listing default everywhere.
    pub(crate) fn to_state(self) -> Option<StickerState> {
        match self {
            Self::All => None,
            Self::Open => Some(StickerState::Open),
            Self::Closed => Some(StickerState::Close),
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

/// Reject `-n 0` at the parser: a listing that cannot show anything is a mistake, not a result.
pub(crate) fn parse_limit(s: &str) -> Result<usize, String> {
    s.parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| "'{s}' is not a positive number of entries".replace("{s}", s))
}

#[derive(clap::Args, Debug)]
pub struct ListArgs {
    /// Which stickers to include
    #[arg(long, value_enum, default_value = "all")]
    pub state: StateFilter,

    /// Only show stickers of this kind
    #[arg(long = "type", value_enum, value_name = "KIND")]
    pub sticker_type: Option<TypeFilter>,

    /// Match against the title and the content
    #[arg(long, short = 's', value_name = "TEXT")]
    pub search: Option<String>,

    /// Show at most this many stickers
    #[arg(long, short = 'n', value_name = "COUNT", value_parser = parse_limit)]
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
        StickerState::Close => "closed",
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

pub(crate) fn run(runtime: &Runtime, args: ListArgs, format: Format) -> anyhow::Result<()> {
    let store = runtime.store()?;
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
            "items": stickers.iter().map(to_json).collect::<Vec<_>>(),
            "count": stickers.len(),
            "total": total,
        }),
        || {
            if stickers.is_empty() {
                println!("No stickers match.");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_must_be_positive() {
        assert_eq!(parse_limit("1"), Ok(1));
        assert!(parse_limit("0").is_err());
        assert!(parse_limit("-1").is_err());
        assert!(parse_limit("many").is_err());
    }

    #[test]
    fn only_all_clears_the_state_filter() {
        assert_eq!(StateFilter::All.to_state(), None);
        assert_eq!(StateFilter::Open.to_state(), Some(StickerState::Open));
        assert_eq!(StateFilter::Closed.to_state(), Some(StickerState::Close));
    }

    #[test]
    fn closed_is_the_spoken_state_name() {
        assert_eq!(state_name(StickerState::Close), "closed");
        assert_eq!(state_name(StickerState::Open), "open");
    }
}
