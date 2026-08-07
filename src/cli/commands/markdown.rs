//! `rusticker markdown` — create a markdown note sticker.

use anyhow::{Context as _, bail};
use serde_json::json;
use std::io::Read as _;

use crate::model::sticker::{StickerColor, StickerType};
use crate::storage::paths::AppPaths;

use crate::cli::draft::{Geometry, StickerDraft};
use crate::cli::output::{Format, ellipsize};
use crate::cli::runtime::open_store;

#[derive(clap::Args, Debug)]
pub struct MarkdownArgs {
    /// Title for the sticker; defaults to the first non-empty line of the content
    #[arg(long, short = 't', value_name = "TEXT")]
    pub title: Option<String>,

    /// Markdown content
    #[arg(long, short = 'c', value_name = "TEXT", conflicts_with = "file")]
    pub content: Option<String>,

    /// Read the content from a file, or from standard input when given "-"
    ///
    /// Easier than quoting a long document on the command line.
    #[arg(long, short = 'f', value_name = "PATH")]
    pub file: Option<String>,

    #[command(flatten)]
    pub geometry: Geometry,
}

impl MarkdownArgs {
    fn read_content(&self) -> anyhow::Result<String> {
        match (&self.content, &self.file) {
            (Some(content), _) => Ok(content.clone()),
            (None, Some(path)) if path == "-" => {
                let mut buffer = String::new();
                std::io::stdin()
                    .read_to_string(&mut buffer)
                    .context("failed to read the content from standard input")?;
                Ok(buffer)
            }
            (None, Some(path)) => std::fs::read_to_string(path)
                .with_context(|| format!("failed to read the content from '{path}'")),
            (None, None) => Ok(String::new()),
        }
    }
}

/// Fall back to the first line that has something on it, which is almost always the heading.
fn derive_title(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| ellipsize(line.trim_start_matches(['#', ' ']), 40))
        .unwrap_or_else(|| "Markdown".to_owned())
}

pub fn run(app_paths: &AppPaths, args: MarkdownArgs, format: Format) -> anyhow::Result<()> {
    let content = args.read_content()?;
    if args.file.is_some() && content.trim().is_empty() {
        bail!("the content source is empty");
    }

    let title = args.title.clone().unwrap_or_else(|| derive_title(&content));

    let store = open_store(app_paths)?;
    let created = StickerDraft {
        title: title.clone(),
        sticker_type: StickerType::Markdown,
        content,
        default_color: StickerColor::Yellow,
        default_width: 400,
        default_height: 300,
    }
    .create(&store, &args.geometry)?;

    let mut payload = created.json("markdown");
    payload["title"] = json!(title);

    format.emit(payload, || {
        created.report(format, "markdown");
        println!("  title: {title}");
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_comes_from_the_first_heading() {
        assert_eq!(derive_title("# Shopping list\n\n- milk"), "Shopping list");
    }

    #[test]
    fn leading_blank_lines_are_skipped() {
        assert_eq!(derive_title("\n\n  hello  \nworld"), "hello");
    }

    #[test]
    fn empty_content_gets_a_placeholder_title() {
        assert_eq!(derive_title(""), "Markdown");
        assert_eq!(derive_title("\n \n"), "Markdown");
    }

    #[test]
    fn a_long_heading_is_shortened() {
        let title = derive_title(&format!("# {}", "x".repeat(80)));
        assert_eq!(title.chars().count(), 41);
        assert!(title.ends_with('…'));
    }
}
