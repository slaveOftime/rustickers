//! Opening, finding and closing sticker windows.
//!
//! Persisted sticker windows and file previews are tracked in [`OPEN_STICKERS`] under an *open id*:
//! the sticker's database id for a saved sticker, or a hash of the previewed sources for an unsaved
//! file preview. Throwaway selection runs are deliberately not tracked, so several can stay open.
//!
//! Opening an id that already has a window raises that window rather than making a second one.
//! Previews therefore stack up freely — each distinct set of files gets its own window, and
//! asking for the same files twice is idempotent.

use std::{
    path::PathBuf,
    sync::{
        RwLock,
        atomic::{AtomicI64, Ordering},
        mpsc,
    },
};

use crate::gpui_component::Root;
use gpui::{
    AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Pixels, Point, Size, Styled,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, point, px, size,
};
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

static NEXT_SELECTION_RUN_OPEN_ID: AtomicI64 = AtomicI64::new(i64::MAX);

/// Optional tweaks for a file preview window.
#[derive(Default)]
pub struct PreviewOptions {
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub color: Option<StickerColor>,
    /// Flash the window background in the sticker color.
    pub flash: bool,
}

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
    /// Flash the window background in the sticker color.
    pub(super) flash: bool,
    /// When true, ignore the saved per-monitor placement and open the window centered on the
    /// primary display, keeping only the saved size. Used when reopening a sticker that was
    /// saved as [`StickerState::Close`]: the position it last lived at is stale, but its size is
    /// still meaningful.
    pub(super) preserve_size_only: bool,
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
        if cx.update(|cx| -> anyhow::Result<bool> {
            if let Some(handle) = find_open_window(id) {
                handle.update(cx, |_, window, cx| {
                    window.activate_window();
                    if open_in_settings
                        && let Some(Some(root)) = window.root::<Root>()
                        && let Ok(entity) = root.read(cx).view().clone().downcast::<StickerWindow>()
                    {
                        entity.update(cx, |this, cx| this.view.begin_edit(window, cx));
                    }
                })?;
                return Ok(true);
            }
            Ok(false)
        })? {
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
            PreviewOptions::default(),
        )
    }

    pub fn open_file_preview_with_sources(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        sources: Vec<String>,
        options: PreviewOptions,
    ) -> anyhow::Result<()> {
        // Previews are keyed by the sources they show, so asking for the same ones again raises
        // the window that already has them instead of opening a duplicate.
        let open_id = preview_open_id(&sources);
        if activate_open_window(open_id, cx)? {
            return Ok(());
        }

        let default_size = FileSticker::default_window_size_for_sources(
            &sources.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        );

        let title = if sources.len() == 1 {
            source_title(&sources[0])
        } else {
            format!("{} files", sources.len())
        };

        let width = options.width.unwrap_or(default_size.width);
        let height = options.height.unwrap_or(default_size.height);

        let origin = free_preview_origin(cx, size(width, height));

        let detail = StickerDetail {
            id: open_id,
            title,
            state: StickerState::Open,
            left: origin.x,
            top: origin.y,
            width,
            height,
            top_most: true,
            color: options.color.unwrap_or(StickerColor::Gray),
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

        let open_options = OpenOptions {
            focus: true,
            open_id: detail.id,
            flash: options.flash,
            ..Default::default()
        };
        Self::open_with_options(cx, sticker_events_tx, store, detail, open_options)
    }

    /// Discard clean unlocked views after key rotation without saving stale ciphertext.
    pub(crate) fn invalidate_protected_views(cx: &mut App) {
        let handles = OPEN_STICKERS
            .read()
            .map(|rows| rows.clone())
            .unwrap_or_default();
        for (_, handle) in handles {
            let _ = handle.update(cx, |_, window, cx| {
                if let Some(Some(root)) = window.root::<Root>()
                    && let Ok(entity) = root.read(cx).view().clone().downcast::<StickerWindow>()
                {
                    entity.update(cx, |this, cx| {
                        this.view.relock_protected_content(window, cx)
                    });
                }
            });
        }
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
        // Reopening a sticker the database still records as closed: drop its old position and
        // display, keep only the size, and rely on `open_with_options` to land it on the current
        // primary display.
        let preserve_size_only = detail.state == StickerState::Close;
        let options = OpenOptions {
            focus,
            open_id: detail.id,
            open_in_settings,
            hidden,
            preserve_size_only,
            ..Default::default()
        };
        Self::open_with_options(cx, sticker_events_tx, store, detail, options)
    }

    /// Open a throwaway window that runs a command against the current text selection.
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
            flash: false,
            preserve_size_only: false,
        };
        Self::open_with_options(cx, sticker_events_tx, store, detail, options)
    }

    fn open_with_options(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        mut detail: StickerDetail,
        options: OpenOptions,
    ) -> anyhow::Result<()> {
        let OpenOptions {
            focus,
            open_id,
            selection_run,
            hidden,
            preserve_size_only,
            ..
        } = options;

        if !selection_run && activate_open_window(open_id, cx)? {
            return Ok(());
        }

        let min_size = min_window_size(detail.sticker_type);
        let current_size = if detail.width > 0 && detail.height > 0 {
            size(detail.width, detail.height)
        } else {
            default_window_size(detail.sticker_type)
        };
        let mut bounds = Bounds::new(
            gpui::point(px(detail.left as f32), px(detail.top as f32)),
            current_size.map(|x| px(x as f32)),
        );

        // When reopening a sticker the database still records as closed, drop its per-monitor
        // placement so `resolved_placement` and the deferred native correction below can not pull
        // it back to where it last lived. The saved size is what we want to preserve.
        if preserve_size_only {
            forget_saved_placement(&mut detail);
        }

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
        if preserve_size_only {
            bounds = cx
                .primary_display()
                .map(|display| {
                    let visible = display.visible_bounds();
                    Bounds::new(center_within(visible, bounds.size), bounds.size)
                })
                .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), bounds.size));
        } else {
            bounds = match (&resolved, target_display) {
                (Some(resolved), Some(display)) if !resolved.exact => {
                    Bounds::new(display.visible_bounds().origin, bounds.size)
                }
                _ => bounds,
            };
        }

        let top_most = detail.top_most;
        let transient_topmost = top_most && (selection_run || detail.id <= 0);
        let virtual_desktop_id = detail.virtual_desktop_id.clone();
        let restore_rect = if preserve_size_only {
            None
        } else {
            resolved.map(|resolved| resolved.rect)
        };

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
                // Component Root defaults to an opaque theme background. Sticker
                // content owns its translucent surface; keep the overlay root clear.
                cx.new(|cx| Root::new(entity, window, cx).bg(gpui::transparent_black()))
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

        if !selection_run && let Ok(mut open_stickers) = OPEN_STICKERS.write() {
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

/// Raise the window a sticker already has. Returns whether there was one.
fn activate_open_window(open_id: i64, cx: &mut App) -> anyhow::Result<bool> {
    let Some(handle) = find_open_window(open_id) else {
        return Ok(false);
    };
    handle.update(cx, |_, window, _| window.activate_window())?;
    Ok(true)
}

/// How far each additional preview window is nudged away from the one it would otherwise cover.
const PREVIEW_CASCADE_STEP: i32 = 32;

/// How many times a preview is nudged before it gives up and overlaps an existing window.
const PREVIEW_CASCADE_ATTEMPTS: i32 = 12;

/// Where to put a new preview window of `window_size`: centred on the primary display, then
/// cascaded down and to the right until it no longer lands on top of an open sticker.
///
/// Previews of the same kind of file get the same default size, so without this the second one
/// would open exactly behind the first and look as if nothing had happened.
fn free_preview_origin(cx: &mut App, window_size: Size<i32>) -> Point<i32> {
    let screen_size = primary_display_size(cx);
    let centered = point(
        (screen_size.width - window_size.width) / 2,
        (screen_size.height - window_size.height) / 2,
    );
    let bottom_right_limit = point(
        (screen_size.width - window_size.width).max(centered.x),
        (screen_size.height - window_size.height).max(centered.y),
    );

    cascade(centered, bottom_right_limit, &open_window_origins(cx))
}

/// Offset `origin` diagonally, without passing `limit`, until it clears every corner in `taken`.
///
/// Falls back to `origin` once the cascade runs out of room, since overlapping is better than
/// pushing every further preview into the same corner of the screen.
fn cascade(origin: Point<i32>, limit: Point<i32>, taken: &[Point<i32>]) -> Point<i32> {
    (0..PREVIEW_CASCADE_ATTEMPTS)
        .map(|attempt| {
            let offset = attempt * PREVIEW_CASCADE_STEP;
            point(
                (origin.x + offset).min(limit.x),
                (origin.y + offset).min(limit.y),
            )
        })
        .find(|candidate| {
            !taken.iter().any(|taken| {
                (taken.x - candidate.x).abs() < PREVIEW_CASCADE_STEP
                    && (taken.y - candidate.y).abs() < PREVIEW_CASCADE_STEP
            })
        })
        .unwrap_or(origin)
}

/// The top-left corner of every sticker window that is open right now.
fn open_window_origins(cx: &mut App) -> Vec<Point<i32>> {
    let handles: Vec<AnyWindowHandle> = OPEN_STICKERS
        .read()
        .map(|open_stickers| open_stickers.iter().map(|(_, handle)| *handle).collect())
        .unwrap_or_default();

    handles
        .into_iter()
        .filter_map(|handle| {
            handle
                .update(cx, |_, window, _| {
                    let bounds = window.bounds();
                    point(bounds.left().to_f64() as i32, bounds.top().to_f64() as i32)
                })
                .ok()
        })
        .collect()
}

fn primary_display_size(cx: &App) -> Size<i32> {
    cx.primary_display()
        .map(|display| display.bounds().size.map(|pixels| pixels.to_f64() as i32))
        .unwrap_or(size(1920, 1080))
}

/// Strip a sticker's per-monitor placement memory so it reopens somewhere chosen by the caller
/// instead of being pulled back to where it last lived.
///
/// `width` and `height` are kept so a sticker that was resized stays the size the user left it at.
/// `virtual_desktop_id` is also kept because that is unrelated to position. `placements` is the
/// source of truth used by [`super::bounds::resolved_placement`], and `native_left/top/width/height`
/// are the fallback inline fields older stickers carry for the same purpose.
fn forget_saved_placement(detail: &mut StickerDetail) {
    detail.placements.clear();
    detail.preferred_display_uuid = None;
    detail.display_id = None;
    detail.display_uuid = None;
    detail.native_left = None;
    detail.native_top = None;
    detail.native_width = None;
    detail.native_height = None;
}

/// Top-left coordinate that centers `size` inside `area`.
///
/// Negative origins (a monitor placed above or to the left of the primary) shift the centered
/// result by the same amount, so a window stays inside the monitor it was centered on. When
/// `size` is larger than `area`, it pins to the area's origin so the window remains reachable.
fn center_within(area: Bounds<Pixels>, size: Size<Pixels>) -> Point<Pixels> {
    let slack_x = (area.size.width - size.width).max(px(0.)) * 0.5_f32;
    let slack_y = (area.size.height - size.height).max(px(0.)) * 0.5_f32;
    point(area.left() + slack_x, area.top() + slack_y)
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

/// The open id of the preview for `sources`.
///
/// Previews are never saved, so they have no database id to be keyed by. Hashing their sources
/// instead gives every distinct set of files its own window while asking for the same files twice
/// lands on the window that already shows them.
fn preview_open_id(sources: &[String]) -> i64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    sources.hash(&mut hasher);
    // Shifting rather than negating keeps this in range: `-i64::MIN` would overflow.
    -1 - (hasher.finish() >> 1) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::sticker::StickerPlacement;
    use crate::native::windows::placement::DisplayEntry;

    fn sources(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    #[test]
    fn preview_ids_are_stable_negative_and_source_specific() {
        let one = preview_open_id(&sources(&["a.md"]));
        let again = preview_open_id(&sources(&["a.md"]));
        let other = preview_open_id(&sources(&["b.md"]));

        assert_eq!(one, again, "the same sources must reuse one window");
        assert_ne!(one, other, "different sources must get their own window");
        assert!(one < 0 && other < 0, "previews must not claim database ids");
    }

    #[test]
    fn preview_id_never_overflows_on_an_extreme_hash() {
        // Guards the `-1 - (hash >> 1)` form: negating `i64::MIN` directly would panic.
        for sources in [sources(&[]), sources(&[""]), sources(&["a", "b", "c"])] {
            assert!(preview_open_id(&sources) < 0);
        }
    }

    #[test]
    fn cascade_keeps_the_first_window_centered() {
        let origin = point(100, 100);
        assert_eq!(cascade(origin, point(900, 900), &[]), origin);
    }

    #[test]
    fn cascade_steps_past_windows_that_are_already_there() {
        let origin = point(100, 100);
        let taken = vec![origin, point(132, 132)];

        assert_eq!(cascade(origin, point(900, 900), &taken), point(164, 164));
    }

    #[test]
    fn cascade_ignores_windows_that_are_out_of_the_way() {
        let origin = point(100, 100);
        let taken = vec![point(600, 100), point(100, 600)];

        assert_eq!(cascade(origin, point(900, 900), &taken), origin);
    }

    #[test]
    fn cascade_falls_back_to_the_origin_once_it_runs_out_of_room() {
        let origin = point(100, 100);
        // A limit equal to the origin pins every candidate onto the taken spot.
        assert_eq!(cascade(origin, origin, &[origin]), origin);
    }

    fn closed_detail_with_position(
        rect: (i32, i32),
        size: (i32, i32),
        display: (&str, i64),
    ) -> StickerDetail {
        StickerDetail {
            id: 7,
            title: "memo".into(),
            state: StickerState::Close,
            left: rect.0,
            top: rect.1,
            width: size.0,
            height: size.1,
            top_most: false,
            color: StickerColor::Gray,
            sticker_type: StickerType::Markdown,
            content: String::new(),
            created_at: 0,
            updated_at: 0,
            display_id: Some(display.1),
            display_uuid: Some(display.0.into()),
            virtual_desktop_id: None,
            native_left: Some(rect.0),
            native_top: Some(rect.1),
            native_width: Some(size.0),
            native_height: Some(size.1),
            preferred_display_uuid: Some(display.0.into()),
            placements: vec![StickerPlacement {
                display_uuid: display.0.into(),
                display_id: Some(display.1),
                native_left: rect.0,
                native_top: rect.1,
                native_width: size.0,
                native_height: size.1,
                scale_factor: 1.0,
                updated_at: 1,
            }],
        }
    }

    #[test]
    fn forget_saved_placement_keeps_size_and_strips_position() {
        let mut detail = closed_detail_with_position((500, 250), (300, 200), ("display-a", 11));
        let title = detail.title.clone();

        forget_saved_placement(&mut detail);

        assert_eq!(detail.width, 300);
        assert_eq!(detail.height, 200);
        assert!(detail.placements.is_empty());
        assert!(detail.preferred_display_uuid.is_none());
        assert!(detail.display_id.is_none());
        assert!(detail.display_uuid.is_none());
        assert!(detail.native_left.is_none());
        assert!(detail.native_top.is_none());
        assert!(detail.native_width.is_none());
        assert!(detail.native_height.is_none());
        // Pure stripping: state, title and type are unrelated to placement.
        assert_eq!(detail.state, StickerState::Close);
        assert_eq!(detail.title, title);
        assert_eq!(detail.sticker_type, StickerType::Markdown);
    }

    #[test]
    fn forget_saved_placement_makes_resolved_reopen_no_op_despite_connected_displays() {
        // Even when a preferred monitor is plugged in, a sticker whose memory has been stripped
        // must resolve to nothing so the open path is free to center it on the primary display.
        let preferred = DisplayEntry {
            uuid: "right".into(),
            display_id: Some(2),
            scale_factor: 1.5,
            work_area: (1920, -1200, 4480, 0),
            is_primary: false,
        };
        let primary = DisplayEntry {
            uuid: "primary".into(),
            display_id: Some(1),
            scale_factor: 1.0,
            work_area: (0, 0, 1920, 1040),
            is_primary: true,
        };
        let displays = [primary.clone(), preferred.clone()];

        let mut detail = closed_detail_with_position((2300, -1000), (315, 171), ("right", 2));
        forget_saved_placement(&mut detail);

        assert!(resolved_placement(&detail, &displays).is_none());
    }

    #[test]
    fn center_within_aligns_a_smaller_window_in_the_middle() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1080.)));
        let window = size(px(300.), px(200.));

        assert_eq!(center_within(area, window), point(px(810.), px(440.)));
    }

    #[test]
    fn center_within_pins_a_window_larger_than_the_area_to_the_origin() {
        let area = Bounds::new(point(px(0.), px(0.)), size(px(100.), px(100.)));
        let window = size(px(200.), px(200.));

        assert_eq!(center_within(area, window), point(px(0.), px(0.)));
    }

    #[test]
    fn center_within_respects_a_negative_origin_for_an_above_or_left_monitor() {
        // A second monitor above and to the right of the primary.
        let area = Bounds::new(point(px(1920.), px(-1200.)), size(px(2560.), px(1400.)));
        let window = size(px(600.), px(400.));

        let centered = center_within(area, window);
        // x = 1920 + (2560 - 600) / 2 = 1920 + 980
        // y = -1200 + (1400 - 400) / 2 = -1200 + 500
        assert_eq!(centered, point(px(2900.), px(-700.)));
    }
}
