//! `rst list` — find stickers.

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
    #[arg(long, value_enum)]
    pub state: Option<StateFilter>,

    /// Only show stickers of this kind
    #[arg(long = "type", value_enum, value_name = "KIND")]
    pub sticker_type: Option<TypeFilter>,

    /// Match against the title and the content
    #[arg(long, short = 's', value_name = "TEXT")]
    pub search: Option<String>,

    /// Show at most this many stickers
    #[arg(long, short = 'n', value_name = "COUNT")]
    pub limit: Option<usize>,

    /// Include ordinary text stickers (open and closed by default)
    #[arg(long)]
    pub text: bool,

    /// Include disposable clipboard captures
    #[arg(long)]
    pub cp: bool,

    /// Use one text entry without choosing: sticker:42 or clipboard:42
    #[arg(long, conflicts_with_all = ["search", "limit"], requires = "command")]
    pub id: Option<crate::text::TextEntryId>,

    /// Select text and execute a program; replace {{txt}} in argument tokens
    #[arg(last = true, num_args = 1.., conflicts_with = "limit")]
    pub command: Vec<String>,
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

fn run_text(paths: &AppPaths, args: ListArgs, format: Format) -> anyhow::Result<()> {
    anyhow::ensure!(
        args.sticker_type.is_none() || args.sticker_type == Some(TypeFilter::Markdown),
        "text selection only supports Markdown stickers; omit --type"
    );
    anyhow::ensure!(
        args.command.is_empty() || !format.is_json(),
        "--json cannot be combined with child execution"
    );
    let repository = block_on(crate::text::TextRepository::open(&paths.db_path))?;
    let sources = crate::text::TextSources::new(args.text, args.cp);
    let state = args.state.and_then(StateFilter::to_state);
    if !args.command.is_empty() {
        return super::text_execution::run(
            &repository,
            sources,
            state,
            args.id,
            args.search.as_deref().unwrap_or(""),
            &args.command,
        );
    }
    let mut entries = block_on(repository.snapshot(sources, args.search.as_deref().unwrap_or("")))?;
    entries.retain(|e| state.is_none() || e.state().is_none() || e.state() == state);
    let total = entries.len();
    if let Some(limit) = args.limit {
        entries.truncate(limit);
    }
    format.emit(
        json!({"items": entries, "count": entries.len(), "total": total}),
        || {
            if entries.is_empty() {
                println!("No texts match.");
                return;
            }
            let rows = entries
                .iter()
                .map(|entry| {
                    vec![
                        entry.id().to_string(),
                        if entry.is_encrypted() {
                            "locked".to_owned()
                        } else {
                            "plain".to_owned()
                        },
                        entry
                            .state()
                            .map(state_name)
                            .unwrap_or("capture")
                            .to_owned(),
                        one_line(entry.preview(), 72),
                    ]
                })
                .collect::<Vec<_>>();
            table(&["ID", "PROTECTION", "STATE", "TEXT"], &rows);
        },
    );
    Ok(())
}

pub fn run(app_paths: &AppPaths, args: ListArgs, format: Format) -> anyhow::Result<()> {
    if args.text || args.cp {
        return run_text(app_paths, args, format);
    }
    anyhow::ensure!(
        args.command.is_empty() && args.id.is_none(),
        "child execution requires --text and/or --cp"
    );
    let store = open_store(app_paths)?;
    let mut stickers = block_on(store.list_stickers(
        args.state.unwrap_or(StateFilter::Open).to_state(),
        args.search.clone(),
    ))?;

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
                if args.state.unwrap_or(StateFilter::Open) == StateFilter::Open
                    && args.search.is_none()
                {
                    println!(
                        "No open stickers. Try `rst list --state all`, or `rst skill \
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
