use gpui::{Context, Window, div, prelude::*, relative};
use gpui_component::{Icon, green_500, h_flex, v_flex};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use crate::native::components::IconName;

const UNKNOWN_TEXT: &str = "Unknown";

pub(super) struct FileSummary {
    pub(super) name: String,
    pub(super) path: String,
    pub(super) is_dir: bool,
    pub(super) size_bytes: Option<u64>,
    pub(super) file_count: Option<u64>,
    pub(super) folder_count: Option<u64>,
    pub(super) modified_at_ms: Option<i64>,
}

impl FileSummary {
    pub(super) fn new(source: &str) -> Self {
        Self {
            name: super::utils::source_name_for_display(source),
            path: source.to_string(),
            is_dir: false,
            size_bytes: None,
            file_count: None,
            folder_count: None,
            modified_at_ms: None,
        }
    }
}

struct DirectoryStats {
    size_bytes: u64,
    file_count: u64,
    folder_count: u64,
}

impl super::FileSticker {
    pub(super) fn spawn_refresh_summaries(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        self.error = None;
        cx.notify();

        let entity = cx.entity();
        let source_paths = self.source_paths.clone();
        window
            .spawn(cx, async move |cx| {
                let summaries = build_summaries(source_paths);
                let _ = entity.update_in(cx, move |this, _window, cx| {
                    this.summaries = summaries;
                    this.refreshing = false;
                    cx.notify();
                });
            })
            .detach();
    }

    pub(super) fn summary_view(&self) -> gpui::AnyElement {
        let show_size_share = self.summaries.len() > 1;
        let total_known_size = if show_size_share {
            self.summaries
                .iter()
                .filter_map(|item| item.size_bytes)
                .sum::<u64>()
        } else {
            0
        };

        v_flex()
            .when(self.refreshing, |view| {
                view.child(
                    div()
                        .p_2()
                        .text_xs()
                        .opacity(0.85)
                        .child("Refreshing summaries..."),
                )
            })
            .children(self.summaries.iter().map(|item| {
                let size_text = item
                    .size_bytes
                    .map(super::utils::format_size)
                    .unwrap_or_else(|| UNKNOWN_TEXT.to_owned());
                let modified_text = item
                    .modified_at_ms
                    .map(crate::utils::time::format_unix_millis)
                    .unwrap_or_else(|| UNKNOWN_TEXT.to_owned());
                let size_share_percent = if show_size_share && total_known_size > 0 {
                    item.size_bytes
                        .map(|size| size as f32 / total_known_size as f32)
                } else {
                    None
                };
                let size_share_text =
                    size_share_percent.map(|percent| format!("{:.0}%", percent * 100.0));
                let items_text = if item.is_dir {
                    match (item.file_count, item.folder_count) {
                        (Some(file_count), Some(folder_count)) => {
                            Some(format!("{file_count} files, {folder_count} folders"))
                        }
                        _ => Some(UNKNOWN_TEXT.to_string()),
                    }
                } else {
                    None
                };

                div()
                    .p_2()
                    .rounded_md()
                    .relative()
                    .when_some(size_share_percent, |view, p| {
                        view.child(
                            div()
                                .bg(green_500().alpha(0.2))
                                .w(relative(p))
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0(),
                        )
                    })
                    .child(
                        v_flex()
                            .gap_1()
                            .text_sm()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .when(item.is_dir, |view| {
                                        view.child(Icon::new(IconName::Folder))
                                    })
                                    .child(item.name.clone()),
                            )
                            .child(div().text_xs().opacity(0.9).child(item.path.clone()))
                            .child(
                                h_flex()
                                    .text_xs()
                                    .opacity(0.8)
                                    .gap_3()
                                    .flex_wrap()
                                    .child(size_text.to_string())
                                    .when_some(size_share_text, |view, text| view.child(text))
                                    .when_some(items_text, |view, text| view.child(text))
                                    .child(format!("Modified: {modified_text}")),
                            ),
                    )
            }))
            .into_any_element()
    }
}

fn build_summaries(source_paths: Vec<String>) -> Vec<FileSummary> {
    let mut summaries = Vec::with_capacity(source_paths.len());
    for raw_path in source_paths {
        if crate::utils::url::is_url(raw_path.as_str()) {
            summaries.push(FileSummary::new(raw_path.as_str()));
            continue;
        }
        let path = PathBuf::from(raw_path.as_str());
        if path.exists() {
            summaries.push(build_summary(path));
        } else {
            summaries.push(FileSummary::new(raw_path.as_str()));
        }
    }
    summaries.sort_by(|a, b| {
        b.size_bytes
            .unwrap_or(0)
            .cmp(&a.size_bytes.unwrap_or(0))
            .then_with(|| a.name.cmp(&b.name))
    });
    summaries
}

fn build_summary(path: PathBuf) -> FileSummary {
    let metadata = std::fs::metadata(&path).ok();
    let is_dir = metadata.as_ref().is_some_and(|m| m.is_dir());
    let dir_stats = if is_dir {
        summarize_directory(path.as_path())
    } else {
        None
    };
    let size_bytes = dir_stats
        .as_ref()
        .map(|stats| stats.size_bytes)
        .or_else(|| metadata.as_ref().map(|m| m.len()));
    let modified_at_ms = metadata
        .and_then(|m| m.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64);
    FileSummary {
        name: super::utils::file_name_for_display(path.as_path()),
        path: path.to_string_lossy().to_string(),
        is_dir,
        size_bytes,
        file_count: dir_stats.as_ref().map(|stats| stats.file_count),
        folder_count: dir_stats.as_ref().map(|stats| stats.folder_count),
        modified_at_ms,
    }
}

fn summarize_directory(root: &Path) -> Option<DirectoryStats> {
    use ignore::{WalkBuilder, WalkState};

    fn atomic_saturating_add(counter: &AtomicU64, value: u64) {
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_add(value))
        });
    }

    let root_path = Arc::new(root.to_path_buf());
    let size_bytes = Arc::new(AtomicU64::new(0));
    let file_count = Arc::new(AtomicU64::new(0));
    let folder_count = Arc::new(AtomicU64::new(0));

    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .require_git(false);

    builder.build_parallel().run(|| {
        let root_path = Arc::clone(&root_path);
        let size_bytes = Arc::clone(&size_bytes);
        let file_count = Arc::clone(&file_count);
        let folder_count = Arc::clone(&folder_count);
        Box::new(move |result| {
            let entry = match result {
                Ok(entry) => entry,
                Err(_) => return WalkState::Continue,
            };
            if entry.path() == root_path.as_path() {
                return WalkState::Continue;
            }
            let file_type = match entry.file_type() {
                Some(file_type) => file_type,
                None => return WalkState::Continue,
            };
            if file_type.is_dir() {
                atomic_saturating_add(&folder_count, 1);
            } else if file_type.is_file() {
                atomic_saturating_add(&file_count, 1);
                if let Ok(metadata) = entry.metadata() {
                    atomic_saturating_add(&size_bytes, metadata.len());
                }
            }
            WalkState::Continue
        })
    });

    Some(DirectoryStats {
        size_bytes: size_bytes.load(Ordering::Relaxed),
        file_count: file_count.load(Ordering::Relaxed),
        folder_count: folder_count.load(Ordering::Relaxed),
    })
}
