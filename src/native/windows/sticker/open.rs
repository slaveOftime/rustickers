//! Opening, finding and closing sticker windows.
//!
//! Every live sticker window is tracked in [`OPEN_STICKERS`] under an *open id*: the sticker's
//! database id for saved stickers, a negative id for unsaved ones, and a large positive id for
//! the throwaway window a selection run opens.

use std::{
    path::PathBuf,
    sync::{
        RwLock,
        atomic::{AtomicI64, Ordering},
        mpsc,
    },
};

use gpui::{
    AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Size, Styled, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions, px, size, transparent_black,
};
use gpui_component::Root;
use url::Url;

use crate::model::content::{CommandContent, FileStickerContent};
use crate::model::sticker::{StickerColor, StickerDetail, StickerState, StickerType};
use crate::native::components::stickers::{
    Sticker, command::CommandSticker, file::FileSticker, markdown::MarkdownSticker,
    paint::PaintSticker, timer::TimerSticker,
};
use crate::native::file_manager;
use crate::native::windows::{
    EscapeDismissTarget, StickerWindowEvent, set_escape_dismiss_target_active,
};
use crate::storage::ArcStickerStore;

use super::StickerWindow;
use super::bounds::resolved_placement;
use super::platform;

static OPEN_STICKERS: RwLock<Vec<(i64, AnyWindowHandle)>> = RwLock::new(Vec::new());

/// Open ids at or above this belong to selection runs, which never touch the database.
const SELECTION_RUN_OPEN_ID_MIN: i64 = i64::MAX / 2;
static NEXT_SELECTION_RUN_OPEN_ID: AtomicI64 = AtomicI64::new(i64::MAX);

/// Extra knobs used when opening a sticker window.
#[derive(Default)]
pub(super) struct OpenOptions {
    pub(super) focus: bool,
    pub(super) selection: Option<String>,
    pub(super) open_id: i64,
    pub(super) selection_run: bool,
    /// Ask the sticker view to start in its settings view instead of running/restoring.
    pub(super) open_in_settings: bool,
    /// Create the window without showing it, the sticker view can reveal it later.
    pub(super) hidden: bool,
}

impl StickerWindow {
    pub async fn open_async(
        cx: &mut AsyncApp,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        id: i64,
    ) -> anyhow::Result<()> {
        Self::open_async_with_options(cx, sticker_events_tx, store, id, false).await
    }

    /// `open_in_settings` asks the sticker view to start in its editing/settings view instead of
    /// auto running or restoring the previous result. It is a hint, each sticker type decides
    /// whether it applies.
    pub async fn open_async_with_options(
        cx: &mut AsyncApp,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        id: i64,
        open_in_settings: bool,
    ) -> anyhow::Result<()> {
        if let Some(handle) = find_open_window(id) {
            cx.update(|cx| {
                handle.update(cx, |_, window, _| {
                    window.activate_window();
                })
            })?;
            return Ok(());
        }

        let detail = store
            .get_sticker(id)
            .await
            .map_err(|err| anyhow::anyhow!("Failed to open sticker: {err:#}"))?;

        if detail.state != StickerState::Open
            && let Err(err) = store.update_sticker_state(id, StickerState::Open).await
        {
            return Err(anyhow::anyhow!(
                "Failed to update sticker state to open: {err:#}"
            ));
        }

        cx.update(|cx| {
            Self::open_with_detail_and_options(
                cx,
                sticker_events_tx,
                store,
                detail,
                false,
                open_in_settings,
            )
        })
    }

    /// Preview whatever the active file manager has selected, or the file path or URL on the
    /// clipboard when nothing is selected.
    pub fn open_file_preview(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
    ) -> anyhow::Result<()> {
        let selected_files = file_manager::selected_files_from_active_manager()?;
        let sources: Vec<String> = if selected_files.is_empty() {
            clipboard_preview_source().into_iter().collect()
        } else {
            selected_files
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect()
        };

        if sources.is_empty() {
            return Err(anyhow::anyhow!(
                "No file selected and clipboard does not contain a file path or URL"
            ));
        }

        Self::open_file_preview_with_sources(
            cx,
            sticker_events_tx,
            store,
            sources,
            None,
            None,
            None,
        )
    }

    pub fn open_file_preview_with_sources(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        sources: Vec<String>,
        width: Option<i32>,
        height: Option<i32>,
        color: Option<StickerColor>,
    ) -> anyhow::Result<()> {
        let default_size = FileSticker::default_window_size_for_sources(
            &sources.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        );

        let title = if sources.len() == 1 {
            source_title(&sources[0])
        } else {
            format!("{} files", sources.len())
        };

        let width = width.unwrap_or(default_size.width);
        let height = height.unwrap_or(default_size.height);

        let screen_size = cx
            .primary_display()
            .map(|d| d.bounds().size.map(|p| p.to_f64() as i32))
            .unwrap_or(size(1920, 1080));

        let detail = StickerDetail {
            id: generate_consistence_minus_id(&sources),
            title,
            state: StickerState::Open,
            left: (screen_size.width - width) / 2,
            top: (screen_size.height - height) / 2,
            width,
            height,
            top_most: true,
            color: color.unwrap_or(StickerColor::Yellow),
            sticker_type: StickerType::File,
            content: FileStickerContent::from_sources(&sources).to_json(),
            created_at: 0,
            updated_at: 0,
            display_id: cx.primary_display().map(|x| u64::from(x.id()) as i64),
            display_uuid: None,
            virtual_desktop_id: None,
            native_left: None,
            native_top: None,
            native_width: None,
            native_height: None,
            preferred_display_uuid: None,
            placements: Vec::new(),
        };

        Self::open_with_detail(cx, sticker_events_tx, store, detail, true)
    }

    pub fn try_close(id: i64, cx: &mut App) -> bool {
        tracing::info!("Trying to close sticker with id: {}", id);
        let Some((open_id, handle)) = OPEN_STICKERS.write().ok().and_then(|mut open_stickers| {
            let pos = open_stickers
                .iter()
                .position(|(open_id, _)| *open_id == id)?;
            Some(open_stickers.remove(pos))
        }) else {
            return false;
        };

        set_escape_dismiss_target_active(EscapeDismissTarget::Sticker(open_id), false);
        handle
            .update(cx, |_, window, _| {
                window.remove_window();
                true
            })
            .unwrap_or(false)
    }

    /// Re-key a window after the sticker it shows was saved for the first time.
    pub fn swap_open_sticker_id(old_id: i64, new_id: i64) {
        if let Ok(mut open_stickers) = OPEN_STICKERS.write()
            && let Some((open_id, _)) = open_stickers
                .iter_mut()
                .find(|(open_id, _)| *open_id == old_id)
        {
            set_escape_dismiss_target_active(EscapeDismissTarget::Sticker(*open_id), false);
            *open_id = new_id;
        }
    }

    pub fn dispatch_escape(open_id: i64, cx: &mut App) -> bool {
        let escape = gpui::Keystroke::parse("escape").expect("escape is a valid GPUI keystroke");
        find_open_window(open_id).is_some_and(|handle| {
            handle
                .update(cx, |_, window, cx| window.dispatch_keystroke(escape, cx))
                .unwrap_or(false)
        })
    }

    pub fn open_with_detail(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        detail: StickerDetail,
        focus: bool,
    ) -> anyhow::Result<()> {
        Self::open_with_detail_and_options(cx, sticker_events_tx, store, detail, focus, false)
    }

    pub fn open_with_detail_and_options(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        detail: StickerDetail,
        focus: bool,
        open_in_settings: bool,
    ) -> anyhow::Result<()> {
        // "Run without window" commands stay hidden unless they fail, the only visible way to
        // open them is from the sticker list, which opens them in their settings view.
        let hidden =
            !open_in_settings && command_content(&detail).is_some_and(|cmd| cmd.run_without_window);
        let options = OpenOptions {
            focus,
            open_id: detail.id,
            open_in_settings,
            hidden,
            ..Default::default()
        };
        Self::open_with_options(cx, sticker_events_tx, store, detail, options)
    }

    /// Open the throwaway window that runs a command against the current text selection. Only one
    /// of these exists at a time, so an older one is closed first.
    pub(crate) fn open_with_selection(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        mut detail: StickerDetail,
        selection: String,
    ) -> anyhow::Result<()> {
        tracing::info!(
            sticker_id = detail.id,
            left = detail.left,
            top = detail.top,
            width = detail.width,
            height = detail.height,
            selection_len = selection.len(),
            "Opening selection command sticker"
        );
        let existing_open_id = OPEN_STICKERS.read().ok().and_then(|open_stickers| {
            open_stickers
                .iter()
                .find(|(open_id, _)| *open_id >= SELECTION_RUN_OPEN_ID_MIN)
                .map(|(open_id, _)| *open_id)
        });
        if let Some(existing_open_id) = existing_open_id {
            Self::try_close(existing_open_id, cx);
            cx.defer(move |cx| {
                if let Err(err) =
                    Self::open_with_selection(cx, sticker_events_tx, store, detail, selection)
                {
                    tracing::warn!(error = ?err, "Failed to replace selection command window");
                }
            });
            return Ok(());
        }

        detail.top_most = true;
        // "Run without window" commands run in a hidden window, it only shows itself when the
        // command failed.
        let hidden = command_content(&detail).is_some_and(|cmd| cmd.run_without_window);
        let options = OpenOptions {
            focus: !hidden,
            selection: Some(selection),
            open_id: NEXT_SELECTION_RUN_OPEN_ID.fetch_sub(1, Ordering::Relaxed),
            selection_run: true,
            open_in_settings: false,
            hidden,
        };
        Self::open_with_options(cx, sticker_events_tx, store, detail, options)
    }

    fn open_with_options(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        detail: StickerDetail,
        options: OpenOptions,
    ) -> anyhow::Result<()> {
        let OpenOptions {
            focus,
            open_id,
            selection_run,
            hidden,
            ..
        } = options;

        // Unsaved stickers share a single window, so opening one closes any other unsaved sticker.
        if !selection_run && open_id <= 0 {
            close_other_unsaved_window(open_id, cx);
        }
        if let Some(handle) = find_open_window(open_id) {
            handle.update(cx, |_, window, _| {
                window.activate_window();
            })?;
            return Ok(());
        }

        let min_size = min_window_size(detail.sticker_type);
        let current_size = if detail.width > 0 && detail.height > 0 {
            size(detail.width, detail.height)
        } else {
            default_window_size(detail.sticker_type)
        };
        let bounds = Bounds::new(
            gpui::point(px(detail.left as f32), px(detail.top as f32)),
            current_size.map(|x| px(x as f32)),
        );

        // Pick the monitor the sticker belongs to: its preferred one when that is plugged in,
        // otherwise the primary one, deriving a placement when there is no memory of the target.
        let resolved = resolved_placement(&detail, &platform::display_snapshot(cx));
        let displays = cx.displays();
        let target_display = resolved.as_ref().and_then(|resolved| {
            displays
                .iter()
                .find(|display| platform::display_uuid(display.as_ref()) == resolved.display_uuid)
        });
        let display_id = target_display
            .map(|display| display.id())
            .or_else(|| cx.primary_display().map(|display| display.id()));
        // GPUI creates the window from logical coordinates, which cannot express a precise
        // position across monitors of differing DPI. Aim at the right monitor here and let the
        // native correction below place the window exactly.
        let bounds = match (&resolved, target_display) {
            (Some(resolved), Some(display)) if !resolved.exact => {
                Bounds::new(display.visible_bounds().origin, bounds.size)
            }
            _ => bounds,
        };

        let top_most = detail.top_most;
        let transient_topmost = top_most && (selection_run || detail.id <= 0);
        let virtual_desktop_id = detail.virtual_desktop_id.clone();
        let restore_rect = resolved.map(|resolved| resolved.rect);

        let handle = cx.open_window(
            WindowOptions {
                focus: focus && !hidden,
                show: !hidden,
                display_id,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(min_size.map(|x| px(x as f32))),
                window_background: WindowBackgroundAppearance::Transparent,
                is_resizable: true,
                kind: if top_most && !transient_topmost {
                    WindowKind::Floating
                } else {
                    WindowKind::Normal
                },
                titlebar: None,
                ..Default::default()
            },
            |window, cx| {
                platform::configure_window(window, top_most);

                let entity = cx.new(|cx| {
                    StickerWindow::new(detail, store, sticker_events_tx, options, window, cx)
                });
                cx.new(|cx| Root::new(entity, window, cx).bg(transparent_black().alpha(0.0)))
            },
        )?;

        // GPUI places the window from logical coordinates, which is not precise enough across
        // monitors, so claim the exact placement back as soon as the window exists.
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, _| {
                platform::restore_virtual_desktop(window, virtual_desktop_id.as_deref());
                if let Some(rect) = restore_rect {
                    platform::apply_window_rect(window, rect);
                }
            });
        });

        if focus && !hidden {
            // Opening a window from the global-hotkey callback can happen while Rustickers is
            // inactive. Defer activation until GPUI has committed the new native window;
            // ordering it during the open_window callback is too early on macOS.
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| {
                    platform::refocus_window(window, top_most);
                    window.refresh();
                    window.activate_window();
                });
            });
        }

        if let Ok(mut open_stickers) = OPEN_STICKERS.write() {
            open_stickers.push((open_id, handle.into()));
        }

        Ok(())
    }
}

fn find_open_window(open_id: i64) -> Option<AnyWindowHandle> {
    OPEN_STICKERS.read().ok().and_then(|open_stickers| {
        open_stickers
            .iter()
            .find(|(id, _)| *id == open_id)
            .map(|(_, handle)| *handle)
    })
}

/// Close the window of any other unsaved sticker; they all share one slot.
fn close_other_unsaved_window(open_id: i64, cx: &mut App) {
    let Some((existing_id, handle)) = OPEN_STICKERS.write().ok().and_then(|mut open_stickers| {
        let pos = open_stickers
            .iter()
            .position(|(existing_id, _)| *existing_id < 0 && *existing_id != open_id)?;
        Some(open_stickers.remove(pos))
    }) else {
        return;
    };

    set_escape_dismiss_target_active(EscapeDismissTarget::Sticker(existing_id), false);
    let _ = handle.update(cx, |_, window, _| {
        window.remove_window();
    });
}

fn min_window_size(sticker_type: StickerType) -> Size<i32> {
    match sticker_type {
        StickerType::Timer => TimerSticker::min_window_size(),
        StickerType::Markdown => MarkdownSticker::min_window_size(),
        StickerType::Command => CommandSticker::min_window_size(),
        StickerType::Paint => PaintSticker::min_window_size(),
        StickerType::File => FileSticker::min_window_size(),
    }
}

pub(super) fn default_window_size(sticker_type: StickerType) -> Size<i32> {
    match sticker_type {
        StickerType::Timer => TimerSticker::default_window_size(),
        StickerType::Markdown => MarkdownSticker::default_window_size(),
        StickerType::Command => CommandSticker::default_window_size(),
        StickerType::Paint => PaintSticker::default_window_size(),
        StickerType::File => FileSticker::default_window_size(),
    }
}

pub(super) fn command_content(detail: &StickerDetail) -> Option<CommandContent> {
    if !matches!(detail.sticker_type, StickerType::Command) {
        return None;
    }
    serde_json::from_str::<CommandContent>(&detail.content).ok()
}

/// A readable window title for a file or URL preview.
fn source_title(source: &str) -> String {
    if let Ok(url) = Url::parse(source) {
        if let Some(last_segment) = url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            && !last_segment.is_empty()
        {
            return last_segment.to_string();
        }

        if let Some(host) = url.host_str()
            && !host.is_empty()
        {
            return host.to_string();
        }

        return source.to_string();
    }

    PathBuf::from(source)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string())
        .unwrap_or_else(|| source.to_string())
}

fn clipboard_preview_source() -> Option<String> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let text = clipboard.get_text().ok()?;
    let trimmed = text.trim();

    if trimmed.is_empty() {
        return None;
    }

    if crate::utils::url::is_url(trimmed) {
        return Some(trimmed.to_string());
    }

    let normalized = trimmed
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(trimmed)
        .to_string();

    PathBuf::from(&normalized).exists().then_some(normalized)
}

/// A stable negative id for an unsaved preview, so reopening the same files reuses its window.
fn generate_consistence_minus_id(sources: &[String]) -> i64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    sources.hash(&mut hasher);
    -(hasher.finish() as i64).abs()
}
