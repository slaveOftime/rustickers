use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Context, IntoElement,
    MouseButton, Render, Rgba, Subscription, Window, WindowBackgroundAppearance, WindowBounds,
    WindowControlArea, WindowKind, WindowOptions, div, prelude::*, px, rgba, size,
    transparent_black,
};
use gpui_component::{
    ActiveTheme, Root,
    alert::Alert,
    button::Button,
    h_flex,
    input::{InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    v_flex,
};
use std::{
    path::PathBuf,
    sync::{
        RwLock,
        atomic::{AtomicI64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use url::Url;

#[cfg(target_os = "macos")]
use cocoa::appkit::{
    NSApplication, NSMainMenuWindowLevel, NSWindow, NSWindowButton, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
#[cfg(target_os = "macos")]
use cocoa::base::{YES, nil};
#[cfg(target_os = "macos")]
use objc::{msg_send, sel, sel_impl};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
#[cfg(target_os = "windows")]
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, RECT, WPARAM},
        Graphics::Gdi::{
            GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        System::Com::{CLSCTX_ALL, CoCreateInstance},
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            Shell::{IVirtualDesktopManager, VirtualDesktopManager},
            WindowsAndMessaging::{
                GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW, GetWindowRect, HWND_NOTOPMOST,
                HWND_TOPMOST, IsWindowVisible, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
                SWP_NOSIZE, SWP_NOZORDER, SendMessageW, SetWindowLongPtrW, SetWindowPos,
                USER_DEFAULT_SCREEN_DPI, WM_DPICHANGED, WS_EX_TOOLWINDOW, WS_SYSMENU,
            },
        },
    },
    core::GUID,
};

use crate::model::content::{CommandContent, FileStickerContent};
use crate::model::sticker::{
    StickerBounds, StickerColor, StickerDetail, StickerPlacement, StickerState, StickerType,
};
use crate::native::components::{
    IconName,
    stickers::{
        command::{CommandSticker, CommandStickerWindowRequest},
        file::FileSticker,
        markdown::MarkdownSticker,
        paint::PaintSticker,
        timer::TimerSticker,
        *,
    },
};
use crate::native::file_manager;
#[cfg(target_os = "windows")]
use crate::native::windows::placement::rect_settled;
use crate::native::windows::{
    EscapeDismissTarget, StickerWindowEvent,
    placement::{DisplayEntry, NativeRect, ResolvedPlacement, resolve_placement},
    set_escape_dismiss_target_active,
    transient_topmost::TransientTopmost,
};
use crate::storage::ArcStickerStore;

const BOUNDS_SAVE_DEBOUNCE: Duration = Duration::from_millis(200);
#[cfg(target_os = "macos")]
const NS_NORMAL_WINDOW_LEVEL: i32 = 0;

static OPEN_STICKERS: RwLock<Vec<(i64, AnyWindowHandle)>> = RwLock::new(Vec::new());
const SELECTION_RUN_OPEN_ID_MIN: i64 = i64::MAX / 2;
static NEXT_SELECTION_RUN_OPEN_ID: AtomicI64 = AtomicI64::new(i64::MAX);

/// How long window moves are ignored after the set of connected monitors changes. Windows shoves
/// windows around itself during a dock or undock, and those moves must not be mistaken for the
/// user deliberately choosing a new monitor.
const DISPLAY_TRANSITION_GRACE: Duration = Duration::from_millis(1000);

/// How many times a placement is re-applied before giving up. Crossing a DPI boundary costs one
/// extra pass, see [`rect_settled`] for why.
#[cfg(target_os = "windows")]
const MAX_PLACEMENT_PASSES: usize = 3;

/// How long after opening a sticker its restored placement keeps being re-asserted. Windows may
/// answer a cross-monitor move asynchronously, and GPUI applies the placement of a window opened
/// hidden only once it is shown, both of which land after the deferred restore has run.
#[cfg(target_os = "windows")]
const RESTORE_SETTLE: Duration = Duration::from_millis(1000);

/// A pending correction of the scale factor GPUI believes a sticker window has.
///
/// GPUI caches the scale factor and only refreshes it while handling `WM_DPICHANGED`, which
/// Windows does not reliably raise when a monitor disappears and the window is herded onto
/// another one. The window then keeps its correct size and position but draws its contents at
/// the scale of the monitor it left, until something else forces a relayout — which is why
/// clicking the sticker used to fix it.
///
/// Raising the message ourselves puts GPUI back in sync, but it has to happen *between* entity
/// updates: GPUI answers the resize by calling back into the window, and that call is silently
/// dropped while the window is already mutably borrowed.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
struct ScaleResync {
    hwnd: isize,
    dpi: u32,
    rect: NativeRect,
}

impl ScaleResync {
    #[cfg(target_os = "windows")]
    fn apply(self) {
        let hwnd = HWND(self.hwnd as *mut _);
        // GPUI reads the scale factor from the message and then moves the window to the rectangle
        // it carries. Ask for a slightly shorter window so that move really produces a `WM_SIZE`,
        // the only thing that makes GPUI adopt the new scale factor, then restore the exact size.
        let mut suggested = RECT {
            left: self.rect.left,
            top: self.rect.top,
            right: self.rect.left + self.rect.width,
            bottom: self.rect.top + self.rect.height - 1,
        };
        unsafe {
            SendMessageW(
                hwnd,
                WM_DPICHANGED,
                Some(WPARAM(((self.dpi << 16) | self.dpi) as usize)),
                Some(LPARAM(&mut suggested as *mut RECT as isize)),
            );
        }
        StickerWindow::apply_native_rect(hwnd, self.rect);
    }

    #[cfg(not(target_os = "windows"))]
    fn apply(self) {}
}

#[derive(PartialEq, Clone, Debug)]
struct WindowState {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    display_id: Option<i64>,
    display_uuid: Option<String>,
    virtual_desktop_id: Option<String>,
    native_left: Option<i32>,
    native_top: Option<i32>,
    native_width: Option<i32>,
    native_height: Option<i32>,
    scale_factor: f32,
}

impl WindowState {
    fn native_rect(&self) -> Option<NativeRect> {
        Some(NativeRect {
            left: self.native_left?,
            top: self.native_top?,
            width: self.native_width?,
            height: self.native_height?,
        })
    }
}

/// Extra knobs used when opening a sticker window.
#[derive(Default)]
struct OpenOptions {
    focus: bool,
    selection: Option<String>,
    open_id: i64,
    selection_run: bool,
    /// Ask the sticker view to start in its settings view instead of running/restoring.
    open_in_settings: bool,
    /// Create the window without showing it, the sticker view can reveal it later.
    hidden: bool,
}

/// A stable key for a monitor. GPUI derives the UUID from the Windows display device name; the
/// raw handle is only a last resort, it changes between sessions.
#[cfg(target_os = "windows")]
fn display_uuid_of(display: &dyn gpui::PlatformDisplay) -> String {
    match display.uuid() {
        Ok(uuid) => uuid.to_string(),
        Err(err) => {
            let raw = u64::from(display.id());
            tracing::warn!(raw, error = ?err, "Monitor has no stable UUID, falling back to its handle");
            format!("id:{raw}")
        }
    }
}

/// A value that changes whenever a monitor is added, removed, moved or rescaled.
fn display_fingerprint(displays: &[DisplayEntry]) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut entries: Vec<&DisplayEntry> = displays.iter().collect();
    entries.sort_by(|a, b| a.uuid.cmp(&b.uuid));

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for entry in entries {
        entry.uuid.hash(&mut hasher);
        entry.work_area.hash(&mut hasher);
        entry.scale_factor.to_bits().hash(&mut hasher);
        entry.is_primary.hash(&mut hasher);
    }
    hasher.finish()
}

/// The per-monitor placements of a sticker, falling back to the single placement older stickers
/// carry inline so they keep working before their first save.
fn placements_of(detail: &StickerDetail) -> Vec<StickerPlacement> {
    if !detail.placements.is_empty() {
        return detail.placements.clone();
    }

    let (Some(display_uuid), Some(left), Some(top), Some(width), Some(height)) = (
        detail.display_uuid.clone(),
        detail.native_left,
        detail.native_top,
        detail.native_width,
        detail.native_height,
    ) else {
        return Vec::new();
    };

    vec![StickerPlacement {
        display_uuid,
        display_id: detail.display_id,
        native_left: left,
        native_top: top,
        native_width: width,
        native_height: height,
        // The logical size was captured next to the native one, so their ratio is the DPI scale
        // of the monitor the rect came from, even once that monitor is gone.
        scale_factor: if detail.width > 0 && width > 0 {
            width as f32 / detail.width as f32
        } else {
            1.0
        },
        updated_at: detail.updated_at,
    }]
}

fn command_content(detail: &StickerDetail) -> Option<CommandContent> {
    if !matches!(detail.sticker_type, StickerType::Command) {
        return None;
    }
    serde_json::from_str::<CommandContent>(&detail.content).ok()
}

pub struct StickerWindow {
    open_id: i64,
    store: ArcStickerStore,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    detail: StickerDetail,

    view: Box<dyn StickerView>,
    error: Option<String>,

    last_bounds: Option<WindowState>,
    last_bounds_change_at: Option<Instant>,
    /// Identifies the monitor layout the sticker was last reconciled against.
    display_fingerprint: u64,
    /// While set, the monitor layout is still settling and window moves are not the user's doing.
    transition_until: Option<Instant>,
    /// The rect this app last forced onto the window. Seeing exactly this rect means the move was
    /// ours, so it must not be recorded as the user choosing a monitor.
    programmatic_rect: Option<NativeRect>,
    /// The placement the window is still being nudged towards right after it opened, together with
    /// the moment that stops. See [`RESTORE_SETTLE`].
    pending_restore: Option<(NativeRect, Instant)>,
    selection_run: bool,
    closing: bool,
    transient_topmost: TransientTopmost,
    protected_relock_generation: u64,
    _window_activation_subscription: Subscription,
}

impl StickerWindow {
    #[cfg(target_os = "macos")]
    fn configure_native_window(window: &Window, top_most: bool) {
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };

        // `Stationary` opts sticker windows out of macOS's Show Desktop animation,
        // so clicking the desktop does not push them toward the screen edges.
        unsafe {
            let view = handle.ns_view.as_ptr() as cocoa::base::id;
            let native_window: cocoa::base::id = msg_send![view, window];
            if !native_window.is_null() {
                // GPUI currently omits NSResizableWindowMask when titlebar is None, even when
                // WindowOptions::is_resizable is true. Sticker windows are borderless, so restore
                // the native resize style explicitly.
                let style = native_window.styleMask();
                native_window.setStyleMask_(style | NSWindowStyleMask::NSResizableWindowMask);

                // Adding the native resize style can make AppKit recreate the standard titlebar
                // controls. Keep native edge resizing but hide the traffic-light buttons.
                for button_kind in [
                    NSWindowButton::NSWindowCloseButton,
                    NSWindowButton::NSWindowMiniaturizeButton,
                    NSWindowButton::NSWindowZoomButton,
                    NSWindowButton::NSWindowFullScreenButton,
                ] {
                    let button = native_window.standardWindowButton_(button_kind);
                    if !button.is_null() {
                        let _: () = msg_send![button, setHidden: YES];
                    }
                }

                let behavior = native_window.collectionBehavior();
                let transient_behavior =
                    NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
                        | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary;
                let mut behavior =
                    behavior | NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary;
                if top_most {
                    behavior |= transient_behavior;
                } else {
                    behavior &= !transient_behavior;
                }
                native_window.setCollectionBehavior_(behavior);
                native_window.setLevel_(if top_most {
                    (NSMainMenuWindowLevel + 1) as _
                } else {
                    NS_NORMAL_WINDOW_LEVEL as _
                });

                if top_most {
                    // GPUI's floating level is only slightly above normal windows. Use the
                    // status-window level and explicitly order the preview to the front so it
                    // appears on the active Space without activating the whole application.
                    // The global hotkey fires while Finder (or another application) is active.
                    // AppKit may defer presentation of a newly created Metal window belonging to
                    // an inactive application until that application receives an event. Activate
                    // Rustickers first, then make this preview the key/front window.
                    let app = NSApplication::sharedApplication(nil);
                    app.activateIgnoringOtherApps_(YES);
                    let _: () = msg_send![native_window, makeKeyAndOrderFront: nil];
                    native_window.orderFrontRegardless();
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    fn native_hwnd(window: &Window) -> Option<HWND> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return None;
        };
        Some(HWND(handle.hwnd.get() as *mut _))
    }

    #[cfg(target_os = "windows")]
    fn virtual_desktop_manager() -> windows::core::Result<IVirtualDesktopManager> {
        unsafe { CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_ALL) }
    }

    #[cfg(target_os = "windows")]
    fn current_virtual_desktop_id(window: &Window) -> Option<String> {
        let hwnd = Self::native_hwnd(window)?;
        let manager = Self::virtual_desktop_manager().ok()?;
        unsafe { manager.GetWindowDesktopId(hwnd) }
            .ok()
            .map(|id| format!("{id:?}"))
    }

    #[cfg(not(target_os = "windows"))]
    fn current_virtual_desktop_id(_window: &Window) -> Option<String> {
        None
    }

    #[cfg(target_os = "windows")]
    fn restore_virtual_desktop(window: &Window, desktop_id: Option<&str>) {
        let Some(desktop_id) = desktop_id.and_then(|id| GUID::try_from(id).ok()) else {
            return;
        };
        let Some(hwnd) = Self::native_hwnd(window) else {
            return;
        };
        let result = Self::virtual_desktop_manager()
            .and_then(|manager| unsafe { manager.MoveWindowToDesktop(hwnd, &desktop_id) });
        if let Err(err) = result {
            tracing::warn!(error = ?err, "Failed to restore sticker virtual desktop");
        }
    }

    #[cfg(target_os = "windows")]
    fn native_rect(window: &Window) -> Option<NativeRect> {
        Self::hwnd_rect(Self::native_hwnd(window)?)
    }

    #[cfg(not(target_os = "windows"))]
    fn native_rect(_window: &Window) -> Option<NativeRect> {
        None
    }

    #[cfg(target_os = "windows")]
    fn monitor_scale_factor(monitor: HMONITOR) -> f32 {
        let mut dpi_x = USER_DEFAULT_SCREEN_DPI;
        let mut dpi_y = USER_DEFAULT_SCREEN_DPI;
        match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } {
            Ok(()) => dpi_x as f32 / USER_DEFAULT_SCREEN_DPI as f32,
            Err(_) => 1.0,
        }
    }

    #[cfg(target_os = "windows")]
    fn work_area(monitor: HMONITOR) -> Option<RECT> {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe { GetMonitorInfoW(monitor, &mut info) }
            .as_bool()
            .then_some(info.rcWork)
    }

    /// The monitors that are connected right now, described the way the placement resolver needs
    /// them: stable UUID, DPI scale and work area in native pixels.
    #[cfg(target_os = "windows")]
    fn display_snapshot(cx: &App) -> Vec<DisplayEntry> {
        let primary = cx.primary_display().map(|display| u64::from(display.id()));
        cx.displays()
            .into_iter()
            .filter_map(|display| {
                let raw = u64::from(display.id());
                let monitor = HMONITOR(raw as _);
                let area = Self::work_area(monitor)?;
                Some(DisplayEntry {
                    uuid: display_uuid_of(display.as_ref()),
                    display_id: Some(raw as i64),
                    scale_factor: Self::monitor_scale_factor(monitor),
                    work_area: (area.left, area.top, area.right, area.bottom),
                    is_primary: primary == Some(raw),
                })
            })
            .collect()
    }

    #[cfg(not(target_os = "windows"))]
    fn display_snapshot(_cx: &App) -> Vec<DisplayEntry> {
        Vec::new()
    }

    /// Where this sticker belongs given the monitors that exist right now.
    fn resolved_placement(
        detail: &StickerDetail,
        displays: &[DisplayEntry],
    ) -> Option<ResolvedPlacement> {
        resolve_placement(
            &placements_of(detail),
            detail.preferred_display_uuid.as_deref(),
            displays,
        )
    }

    /// Put the window exactly where it is asked to go, even across a DPI boundary.
    ///
    /// GPUI creates every window on the primary monitor, so restoring a sticker that lives
    /// elsewhere means moving it between monitors. When their DPI differs Windows raises
    /// `WM_DPICHANGED` while this very `SetWindowPos` is running and GPUI answers it by applying
    /// the rectangle the system suggests, which is our size multiplied by the DPI ratio. Applying
    /// the same rectangle again fixes it: by then the window already sits on the target monitor,
    /// so no further DPI change is raised.
    #[cfg(target_os = "windows")]
    fn apply_native_rect(hwnd: HWND, rect: NativeRect) {
        let mut previous = None;
        for pass in 0..MAX_PLACEMENT_PASSES {
            if let Err(err) = unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    rect.left,
                    rect.top,
                    rect.width,
                    rect.height,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            } {
                tracing::warn!(?rect, error = ?err, "Failed to apply native sticker placement");
                return;
            }

            let Some(actual) = Self::hwnd_rect(hwnd) else {
                return;
            };
            if rect_settled(rect, actual, previous) {
                if pass > 0 {
                    tracing::debug!(
                        ?rect,
                        ?actual,
                        passes = pass + 1,
                        "Re-applied sticker placement after a DPI change"
                    );
                }
                return;
            }
            previous = Some(actual);
        }

        tracing::warn!(
            ?rect,
            ?previous,
            "Gave up applying the native sticker placement"
        );
    }

    #[cfg(target_os = "windows")]
    fn hwnd_rect(hwnd: HWND) -> Option<NativeRect> {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
        Some(NativeRect {
            left: rect.left,
            top: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
        })
    }

    /// Restore the exact pixel placement the sticker had when it was last saved. GPUI recreates
    /// the window using logical coordinates, which loses precision across monitors of differing
    /// DPI, so both the origin and the size are re-applied natively.
    #[cfg(target_os = "windows")]
    fn restore_native_placement(window: &Window, rect: NativeRect) {
        let Some(hwnd) = Self::native_hwnd(window) else {
            return;
        };
        Self::apply_native_rect(hwnd, rect);
    }

    #[cfg(target_os = "windows")]
    fn configure_preview_window(window: &Window, top_most: bool) {
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };

        // Windows excludes tool windows from Win+D's minimize-all operation. Preserve all
        // existing extended styles because GPUI also uses them for rendering and activation.
        unsafe {
            let hwnd = HWND(handle.hwnd.get() as *mut _);
            let extended_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                extended_style | WS_EX_TOOLWINDOW.0 as isize,
            );

            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_SYSMENU.0 as isize));
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );

            let _ = SetWindowPos(
                hwnd,
                Some(if top_most {
                    HWND_TOPMOST
                } else {
                    HWND_NOTOPMOST
                }),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

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
        if let Ok(open_stickers) = OPEN_STICKERS.read() {
            if let Some((_, handle)) = open_stickers.iter().find(|(open_id, _)| *open_id == id) {
                let _ = cx.update(|cx| {
                    handle.update(cx, |_, window, _| {
                        window.activate_window();
                    })
                })?;
                return Ok(());
            }
        }

        let detail = match store.get_sticker(id).await {
            Ok(detail) => detail,
            Err(err) => {
                return Err(anyhow::anyhow!("Failed to open sticker: {err:#}"));
            }
        };

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

    pub fn open_file_preview(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
    ) -> anyhow::Result<()> {
        let selected_files = file_manager::selected_files_from_active_manager()?;
        let sources = if selected_files.is_empty() {
            clipboard_preview_source()
                .map(|source| vec![source])
                .unwrap_or_default()
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
        let left = (screen_size.width - width) / 2;
        let top = (screen_size.height - height) / 2;

        let content = FileStickerContent::from_sources(&sources).to_json();
        let detail = StickerDetail {
            id: generate_consistence_minus_id(&sources),
            title,
            state: StickerState::Open,
            left,
            top,
            width,
            height,
            top_most: true,
            color: color.unwrap_or(StickerColor::Yellow),
            sticker_type: StickerType::File,
            content,
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
        if let Ok(mut open_stickers) = OPEN_STICKERS.write() {
            if let Some(pos) = open_stickers.iter().position(|(open_id, _)| *open_id == id) {
                let (open_id, handle) = open_stickers.remove(pos);
                set_escape_dismiss_target_active(EscapeDismissTarget::Sticker(open_id), false);
                return handle
                    .update(cx, |_, window, _| {
                        window.remove_window();
                        true
                    })
                    .unwrap_or(false);
            }
        }
        false
    }

    pub fn swap_open_sticker_id(old_id: i64, new_id: i64) {
        if let Ok(mut open_stickers) = OPEN_STICKERS.write() {
            if let Some((open_id, _)) = open_stickers
                .iter_mut()
                .find(|(open_id, _)| *open_id == old_id)
            {
                set_escape_dismiss_target_active(EscapeDismissTarget::Sticker(*open_id), false);
                *open_id = new_id;
            }
        }
    }

    pub fn dispatch_escape(open_id: i64, cx: &mut App) -> bool {
        let escape = gpui::Keystroke::parse("escape").expect("escape is a valid GPUI keystroke");
        OPEN_STICKERS
            .read()
            .ok()
            .and_then(|open_stickers| {
                open_stickers
                    .iter()
                    .find(|(id, _)| *id == open_id)
                    .and_then(|(_, handle)| {
                        handle
                            .update(cx, |_, window, cx| window.dispatch_keystroke(escape, cx))
                            .ok()
                    })
            })
            .unwrap_or(false)
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
            selection,
            open_id,
            selection_run,
            open_in_settings,
            hidden,
        } = options;
        if let Ok(mut open_stickers) = OPEN_STICKERS.write() {
            if !selection_run && open_id <= 0 {
                if let Some(pos) = open_stickers
                    .iter()
                    .position(|(existing_id, _)| *existing_id < 0 && *existing_id != open_id)
                {
                    let (existing_id, handle) = open_stickers.remove(pos);
                    set_escape_dismiss_target_active(
                        EscapeDismissTarget::Sticker(existing_id),
                        false,
                    );
                    let _ = handle.update(cx, |_, window, _| {
                        window.remove_window();
                    });
                }
            }

            if let Some((_, handle)) = open_stickers
                .iter()
                .find(|(existing_id, _)| *existing_id == open_id)
            {
                handle.update(cx, |_, window, _| {
                    window.activate_window();
                })?;
                return Ok(());
            }
        }

        let min_size = match detail.sticker_type {
            StickerType::Timer => TimerSticker::min_window_size(),
            StickerType::Markdown => MarkdownSticker::min_window_size(),
            StickerType::Command => CommandSticker::min_window_size(),
            StickerType::Paint => PaintSticker::min_window_size(),
            StickerType::File => FileSticker::min_window_size(),
        };

        let current_size = if detail.width > 0 && detail.height > 0 {
            size(detail.width, detail.height)
        } else {
            match detail.sticker_type {
                StickerType::Timer => TimerSticker::default_window_size(),
                StickerType::Markdown => MarkdownSticker::default_window_size(),
                StickerType::Command => CommandSticker::default_window_size(),
                StickerType::Paint => PaintSticker::default_window_size(),
                StickerType::File => FileSticker::default_window_size(),
            }
        };

        let bounds = Bounds::new(
            gpui::point(px(detail.left as f32), px(detail.top as f32)),
            current_size.map(|x| px(x as f32)),
        );

        // Pick the monitor the sticker belongs to: its preferred one when that is plugged in,
        // otherwise the primary one, deriving a placement when there is no memory of the target.
        let resolved = Self::resolved_placement(&detail, &Self::display_snapshot(cx));
        let displays = cx.displays();
        let target_display = resolved.as_ref().and_then(|resolved| {
            displays
                .iter()
                .find(|display| display_uuid_of(display.as_ref()) == resolved.display_uuid)
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
        #[cfg(target_os = "windows")]
        let virtual_desktop_id = detail.virtual_desktop_id.clone();
        #[cfg(target_os = "windows")]
        let restore_rect = resolved.as_ref().map(|resolved| resolved.rect);

        // There is issue which gpui does not restore exactly with the given bounds especially on other displays
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
                #[cfg(target_os = "macos")]
                StickerWindow::configure_native_window(window, top_most);
                #[cfg(target_os = "windows")]
                StickerWindow::configure_preview_window(window, top_most);

                let entity = cx.new(|cx| {
                    StickerWindow::new(
                        open_id,
                        detail,
                        store,
                        sticker_events_tx,
                        selection,
                        selection_run,
                        open_in_settings,
                        hidden,
                        window,
                        cx,
                    )
                });
                cx.new(|cx| Root::new(entity, window, cx).bg(transparent_black().alpha(0.0)))
            },
        )?;

        #[cfg(target_os = "windows")]
        cx.defer({
            let handle = handle.clone();
            move |cx| {
                let _ = handle.update(cx, |_, window, _| {
                    StickerWindow::restore_virtual_desktop(window, virtual_desktop_id.as_deref());
                    if let Some(rect) = restore_rect {
                        StickerWindow::restore_native_placement(window, rect);
                    }
                });
            }
        });

        if focus && !hidden {
            // Opening a window from the global-hotkey callback can happen while Rustickers is
            // inactive. Defer activation until GPUI has committed the new native window;
            // ordering it during the open_window callback is too early on macOS.
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| {
                    #[cfg(target_os = "macos")]
                    StickerWindow::configure_native_window(window, top_most);
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

    fn new(
        open_id: i64,
        detail: StickerDetail,
        store: ArcStickerStore,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        selection: Option<String>,
        selection_run: bool,
        open_in_settings: bool,
        hidden: bool,
        window: &mut Window,
        cx: &mut Context<StickerWindow>,
    ) -> Self {
        let title_val = detail.title.clone();
        let title = cx.new(|cx| InputState::new(window, cx).default_value(title_val));
        let transient_topmost = TransientTopmost::new(
            detail.top_most && (selection_run || detail.id <= 0),
            !hidden,
        );

        let mut view = Self::create_sticker_view(
            &detail,
            &store,
            selection,
            open_in_settings,
            hidden,
            window,
            cx,
            sticker_events_tx.clone(),
        );

        view.set_color(cx, detail.color);

        cx.subscribe_in(&title, window, |this, input_state, event, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let id = this.view.id(cx);
                let text = input_state.read(cx).value().to_string();
                let store = this.store.clone();
                let events = this.sticker_events_tx.clone();
                cx.spawn(async move |entity, cx| {
                    if let Err(err) = store.update_sticker_title(id, text.clone()).await {
                        let _ = entity.update(cx, |this, cx| {
                            this.set_error(format!("Failed to save title: {err}"), cx);
                        });
                    } else {
                        let _ = events.send(StickerWindowEvent::TitleChanged { id, title: text });
                    }
                })
                .detach();
            }
        })
        .detach();

        let bounds_entity = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                loop {
                    cx.background_executor().timer(BOUNDS_SAVE_DEBOUNCE).await;
                    match bounds_entity
                        .update_in(cx, |this, window, cx| this.tick_bounds_state(window, cx))
                    {
                        Err(_) => break,
                        // Applying this while the window was borrowed above would be ignored, so
                        // it deliberately happens between updates. See [`ScaleResync`].
                        Ok(Some(resync)) => resync.apply(),
                        Ok(None) => {}
                    }
                }
            })
            .detach();

        let entity = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            set_escape_dismiss_target_active(EscapeDismissTarget::Sticker(open_id), false);
            entity
                .update(cx, |this, cx| {
                    if this.closing {
                        true
                    } else {
                        this.close(window, cx);
                        false
                    }
                })
                .unwrap_or(true)
        });

        let own_window_id = gpui::Window::window_handle(window).window_id();
        let entity = cx.weak_entity();
        cx.intercept_keystrokes(move |event, window, cx| {
            if gpui::Window::window_handle(window).window_id() != own_window_id
                || !window.is_window_active()
            {
                return;
            }

            let lock_shortcut =
                event.keystroke.modifiers.control && event.keystroke.key.eq_ignore_ascii_case("l");
            let escape = event.keystroke.key == "escape";
            if !lock_shortcut && !escape {
                return;
            }

            let handled = entity.upgrade().is_some_and(|entity| {
                entity.update(cx, |this, cx| {
                    if lock_shortcut {
                        return this.view.handle_lock_shortcut(window, cx);
                    }
                    if this.view.suppress_window_escape(cx) {
                        return false;
                    }
                    if !this.selection_run && this.view.id(cx) > 0 {
                        return false;
                    }

                    this.close(window, cx);
                    true
                })
            });
            if handled {
                cx.stop_propagation();
                window.prevent_default();
            }
        })
        .detach();

        let escape_target = EscapeDismissTarget::Sticker(open_id);
        let window_activation_subscription =
            cx.observe_window_activation(window, move |this, window, cx| {
                let active = window.is_window_active();
                this.protected_relock_generation = this.protected_relock_generation.wrapping_add(1);
                if !active && this.view.protected_content_visible(cx) {
                    let generation = this.protected_relock_generation;
                    let entity = cx.entity();
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor()
                                .timer(FOCUS_LOSS_RELOCK_DELAY)
                                .await;
                            let _ = entity.update_in(cx, |this, window, cx| {
                                if this.protected_relock_generation == generation
                                    && !window.is_window_active()
                                    && this.view.protected_content_visible(cx)
                                {
                                    this.view.relock_protected_content(window, cx);
                                }
                            });
                        })
                        .detach();
                }
                let closes_on_escape = this.selection_run || this.view.id(cx) <= 0;
                set_escape_dismiss_target_active(escape_target, closes_on_escape && active);
                if this.transient_topmost.update_activation(active) {
                    #[cfg(target_os = "macos")]
                    StickerWindow::configure_native_window(window, false);
                    #[cfg(target_os = "windows")]
                    StickerWindow::configure_preview_window(window, false);
                }
            });

        let displays = Self::display_snapshot(cx);
        // `open_with_options` asks Windows to place the window right after it is created, but that
        // move can still be undone afterwards: a DPI change may be answered asynchronously, and a
        // window opened hidden only gets GPUI's own placement once it is shown. Keep the resolved
        // rect around so the first ticks can re-assert it, and treat it as our own doing so a
        // botched restore is never saved back as if the user had moved the sticker.
        #[cfg(target_os = "windows")]
        let pending_restore = Self::resolved_placement(&detail, &displays)
            .map(|resolved| (resolved.rect, Instant::now() + RESTORE_SETTLE));
        #[cfg(not(target_os = "windows"))]
        let pending_restore: Option<(NativeRect, Instant)> = None;

        Self {
            open_id,
            store,
            detail,
            sticker_events_tx,
            view,
            last_bounds: None,
            last_bounds_change_at: None,
            display_fingerprint: display_fingerprint(&displays),
            transition_until: None,
            programmatic_rect: pending_restore.map(|(rect, _)| rect),
            pending_restore,
            selection_run,
            closing: false,
            error: None,
            transient_topmost,
            protected_relock_generation: 0,
            _window_activation_subscription: window_activation_subscription,
        }
    }

    fn create_sticker_view(
        detail: &StickerDetail,
        store: &ArcStickerStore,
        selection: Option<String>,
        open_in_settings: bool,
        hidden: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    ) -> Box<dyn StickerView> {
        let id = detail.id;
        let color = detail.color;
        let content = detail.content.as_str();
        let store = store.clone();

        match detail.sticker_type {
            StickerType::Timer => Box::new(StickerViewEntity::new(cx.new(|cx| {
                TimerSticker::new(
                    id,
                    color,
                    store,
                    content,
                    window,
                    cx,
                    sticker_events_tx.clone(),
                )
            }))),
            StickerType::Markdown => Box::new(StickerViewEntity::new(cx.new(|cx| {
                MarkdownSticker::new(
                    id,
                    color,
                    store,
                    content,
                    window,
                    cx,
                    sticker_events_tx.clone(),
                )
            }))),
            StickerType::Command => {
                let entity = cx.new(|cx| {
                    CommandSticker::new(
                        id,
                        color,
                        store,
                        detail.title.as_str(),
                        content,
                        window,
                        cx,
                        sticker_events_tx.clone(),
                        selection,
                        open_in_settings,
                        hidden,
                    )
                });

                cx.subscribe_in(
                    &entity,
                    window,
                    |this, _, event: &CommandStickerWindowRequest, window, cx| match event {
                        CommandStickerWindowRequest::Close => this.close(window, cx),
                        CommandStickerWindowRequest::Show => {
                            #[cfg(target_os = "macos")]
                            StickerWindow::configure_native_window(window, this.detail.top_most);
                            window.refresh();
                            // GPUI holds back the placement of a window opened hidden and applies
                            // it here, computed from the primary monitor's scale factor. Claim the
                            // placement back once the window is really on screen.
                            this.rearm_restore(cx);
                            window.activate_window();
                        }
                    },
                )
                .detach();

                Box::new(StickerViewEntity::new(entity))
            }
            StickerType::Paint => {
                Box::new(StickerViewEntity::new(cx.new(|_| {
                    PaintSticker::new(id, color, store, content, sticker_events_tx.clone())
                })))
            }
            StickerType::File => Box::new(StickerViewEntity::new(cx.new(|cx| {
                FileSticker::new(
                    id,
                    color,
                    store,
                    content,
                    window,
                    cx,
                    sticker_events_tx.clone(),
                )
            }))),
        }
    }

    fn set_error(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        cx.notify();
    }

    fn tick_bounds_state(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ScaleResync> {
        if self.view.id(cx) <= 0 {
            return None;
        }

        let displays = Self::display_snapshot(cx);
        let fingerprint = display_fingerprint(&displays);
        if fingerprint != self.display_fingerprint {
            self.display_fingerprint = fingerprint;
            self.transition_until = Some(Instant::now() + DISPLAY_TRANSITION_GRACE);
            self.reconcile_placement(window, cx, &displays);
            let resync = self.redraw_after_relocation(window, cx);
            self.reset_bounds_watch(window, cx);
            return resync;
        }

        if let Some(until) = self.transition_until {
            if Instant::now() < until {
                // Windows is still shuffling windows around; anything we observe now is its doing.
                self.reset_bounds_watch(window, cx);
                return None;
            }
            self.transition_until = None;
            // The layout has settled, so undo whatever Windows did to the window in the meantime.
            self.reconcile_placement(window, cx, &displays);
            let resync = self.redraw_after_relocation(window, cx);
            self.reset_bounds_watch(window, cx);
            return resync;
        }

        if self.settle_restore(window, cx) {
            return None;
        }

        // Cheap safety net: a monitor change Windows reported late would otherwise leave the
        // sticker drawing itself at the wrong scale until the user clicks it.
        if let Some(resync) = Self::scale_resync(window) {
            window.refresh();
            cx.notify();
            self.reset_bounds_watch(window, cx);
            return Some(resync);
        }

        let current = self.current_bounds(window, cx);
        let changed = self
            .last_bounds
            .as_ref()
            .map(|x| x != &current)
            .unwrap_or(true);

        if changed {
            self.last_bounds = Some(current);
            self.last_bounds_change_at = Some(Instant::now());
            return None;
        }

        if self
            .last_bounds_change_at
            .is_some_and(|changed_at| changed_at.elapsed() >= BOUNDS_SAVE_DEBOUNCE)
        {
            self.last_bounds_change_at = None;
            self.change_bounds(window, cx);
        }
        None
    }

    /// Redraw the sticker after it was moved between monitors.
    ///
    /// Relocating the window with `SetWindowPos` changes its size and, when the monitors differ in
    /// DPI, its scale factor. GPUI does not repaint on its own for a move it did not make, so the
    /// window would keep showing content laid out for the monitor the sticker just left.
    fn redraw_after_relocation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ScaleResync> {
        window.refresh();
        cx.notify();
        Self::scale_resync(window)
    }

    /// Tell GPUI about the DPI of the monitor the sticker now lives on.
    ///
    /// GPUI caches a window's scale factor and only refreshes it from `WM_DPICHANGED`. Windows
    /// does not always raise that message when a monitor disappears and the window is herded onto
    /// another one, which leaves GPUI laying the sticker out for the DPI of a monitor it left:
    /// the window has the right size and position but its contents are drawn at the wrong scale.
    /// Raising the message ourselves puts GPUI back in sync, and its handler re-applies the very
    /// rectangle we pass in, so this does not fight the placement.
    #[cfg(target_os = "windows")]
    fn scale_resync(window: &mut Window) -> Option<ScaleResync> {
        let hwnd = Self::native_hwnd(window)?;
        let rect = Self::hwnd_rect(hwnd)?;
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        let scale_factor = Self::monitor_scale_factor(monitor);
        if (window.scale_factor() - scale_factor).abs() < f32::EPSILON {
            return None;
        }

        tracing::debug!(
            stale = window.scale_factor(),
            scale_factor,
            ?rect,
            "Resynchronising the sticker's scale factor with its monitor"
        );
        Some(ScaleResync {
            hwnd: hwnd.0 as isize,
            dpi: (scale_factor * USER_DEFAULT_SCREEN_DPI as f32).round() as u32,
            rect,
        })
    }

    #[cfg(not(target_os = "windows"))]
    fn scale_resync(_window: &mut Window) -> Option<ScaleResync> {
        None
    }

    /// Forget the pending debounce so the next tick compares against where the window is now.
    fn reset_bounds_watch(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.last_bounds = Some(self.current_bounds(window, cx));
        self.last_bounds_change_at = Some(Instant::now());
    }

    /// Move the sticker to wherever the current monitor layout says it belongs: back onto its
    /// preferred monitor when that is plugged in, otherwise onto the primary one.
    #[cfg(target_os = "windows")]
    fn reconcile_placement(
        &mut self,
        window: &Window,
        _cx: &mut Context<Self>,
        displays: &[DisplayEntry],
    ) {
        let Some(hwnd) = Self::native_hwnd(window) else {
            return;
        };
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return;
        }
        let Some(resolved) = Self::resolved_placement(&self.detail, displays) else {
            return;
        };

        self.programmatic_rect = Some(resolved.rect);
        if Self::native_rect(window) == Some(resolved.rect) {
            return;
        }

        tracing::debug!(
            ?resolved,
            "Relocating sticker for the current monitor layout"
        );
        Self::apply_native_rect(hwnd, resolved.rect);
    }

    #[cfg(not(target_os = "windows"))]
    fn reconcile_placement(
        &mut self,
        _window: &Window,
        _cx: &mut Context<Self>,
        _displays: &[DisplayEntry],
    ) {
    }

    /// Hold the freshly opened window on its restored placement for a moment.
    ///
    /// Returns `true` while the placement is still being asserted, so the caller skips its usual
    /// change detection and never persists a rectangle Windows or GPUI imposed on us.
    #[cfg(target_os = "windows")]
    fn settle_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((rect, until)) = self.pending_restore else {
            return false;
        };
        if Instant::now() >= until {
            self.pending_restore = None;
            return false;
        }
        let Some(hwnd) = Self::native_hwnd(window) else {
            self.pending_restore = None;
            return false;
        };
        // A window that is still hidden has not received GPUI's own placement yet, so keep waiting.
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return true;
        }
        if Self::hwnd_rect(hwnd) != Some(rect) {
            tracing::debug!(?rect, "Re-asserting the restored sticker placement");
            Self::apply_native_rect(hwnd, rect);
            self.redraw_after_relocation(window, cx);
        }
        self.programmatic_rect = Some(rect);
        self.reset_bounds_watch(window, cx);
        true
    }

    #[cfg(not(target_os = "windows"))]
    fn settle_restore(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        false
    }

    /// Start asserting the sticker's placement again, for a window GPUI positions late.
    #[cfg(target_os = "windows")]
    fn rearm_restore(&mut self, cx: &mut Context<Self>) {
        let displays = Self::display_snapshot(cx);
        let Some(resolved) = Self::resolved_placement(&self.detail, &displays) else {
            return;
        };
        self.pending_restore = Some((resolved.rect, Instant::now() + RESTORE_SETTLE));
        self.programmatic_rect = Some(resolved.rect);
    }

    #[cfg(not(target_os = "windows"))]
    fn rearm_restore(&mut self, _cx: &mut Context<Self>) {}

    /// A resync raised here is dropped on purpose: rendering is the one moment the window must not
    /// be resized. The polling timer picks the correction up on its next pass.
    fn try_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.last_bounds.is_none() {
            self.last_bounds = Some(self.current_bounds(window, cx));
        }
        let _ = self.tick_bounds_state(window, cx);
    }

    fn current_bounds(&self, window: &Window, cx: &Context<Self>) -> WindowState {
        let bounds = window.bounds();
        let display = window.display(cx);
        let display_id = display.as_ref().map(|x| u64::from(x.id()) as i64);
        // Must match the key `display_snapshot` uses, or placements are saved under one name and
        // looked up under another.
        #[cfg(target_os = "windows")]
        let display_uuid = display
            .as_ref()
            .map(|display| display_uuid_of(display.as_ref()));
        #[cfg(not(target_os = "windows"))]
        let display_uuid = display
            .and_then(|display| display.uuid().ok())
            .map(|uuid| uuid.to_string());
        let virtual_desktop_id = Self::current_virtual_desktop_id(window);
        let native_rect = Self::native_rect(window);
        let native_left = native_rect.map(|rect| rect.left);
        let native_top = native_rect.map(|rect| rect.top);
        let native_width = native_rect.map(|rect| rect.width);
        let native_height = native_rect.map(|rect| rect.height);
        let scale_factor = window.scale_factor() as f32;

        WindowState {
            left: bounds.left().to_f64() as i32,
            top: bounds.top().to_f64() as i32,
            width: bounds.size.width.to_f64() as i32,
            height: bounds.size.height.to_f64() as i32,
            display_id,
            display_uuid,
            virtual_desktop_id,
            native_left,
            native_top,
            native_width,
            native_height,
            scale_factor,
        }
    }

    fn change_bounds(&mut self, window: &Window, cx: &mut Context<Self>) {
        let state = self.current_bounds(window, cx);
        if state.left == self.detail.left
            && state.top == self.detail.top
            && state.width == self.detail.width
            && state.height == self.detail.height
            && state.display_id == self.detail.display_id
            && state.display_uuid == self.detail.display_uuid
            && state.virtual_desktop_id == self.detail.virtual_desktop_id
            && state.native_left == self.detail.native_left
            && state.native_top == self.detail.native_top
            && state.native_width == self.detail.native_width
            && state.native_height == self.detail.native_height
        {
            return;
        }

        self.last_bounds = Some(state.clone());

        let native_rect = state.native_rect();
        // A move we made ourselves keeps the window on screen but says nothing about which monitor
        // the user wants, so it must not touch the per-monitor memory.
        let programmatic = match (self.programmatic_rect, native_rect) {
            (Some(expected), Some(actual)) => expected == actual,
            _ => false,
        };
        if !programmatic {
            self.programmatic_rect = None;
        }

        let placement = match (&state.display_uuid, native_rect) {
            (Some(display_uuid), Some(rect)) if !programmatic => Some(StickerPlacement {
                display_uuid: display_uuid.clone(),
                display_id: state.display_id,
                native_left: rect.left,
                native_top: rect.top,
                native_width: rect.width,
                native_height: rect.height,
                scale_factor: state.scale_factor,
                updated_at: crate::utils::time::now_unix_millis(),
            }),
            _ => None,
        };
        let primary_uuid = Self::display_snapshot(cx)
            .into_iter()
            .find(|display| display.is_primary)
            .map(|display| display.uuid);

        let id = self.view.id(cx);
        let store = self.store.clone();
        let bounds = StickerBounds {
            left: state.left,
            top: state.top,
            width: state.width,
            height: state.height,
            display_id: state.display_id,
            display_uuid: state.display_uuid.clone(),
            virtual_desktop_id: state.virtual_desktop_id.clone(),
            native_left: state.native_left,
            native_top: state.native_top,
            native_width: state.native_width,
            native_height: state.native_height,
        };

        tracing::debug!(programmatic, "Save bounds state: {:?}", &state);

        cx.spawn(async move |this, cx| {
            let mut result = store.update_sticker_bounds(id, bounds).await;

            if let (Ok(()), Some(placement)) = (&result, placement.clone()) {
                let preferred = placement.display_uuid.clone();
                result = store
                    .upsert_sticker_placement(id, placement, primary_uuid.clone())
                    .await;
                if result.is_ok() {
                    result = store
                        .update_sticker_preferred_display(id, Some(preferred))
                        .await;
                }
            }

            match result {
                Err(err) => {
                    let _ = this.update(cx, |this, cx| {
                        this.set_error(format!("Failed to save window bounds: {err}"), cx);
                    });
                }
                Ok(()) => {
                    let _ = this.update(cx, |this, _| {
                        this.detail.left = state.left;
                        this.detail.top = state.top;
                        this.detail.width = state.width;
                        this.detail.height = state.height;
                        this.detail.display_id = state.display_id;
                        this.detail.display_uuid = state.display_uuid;
                        this.detail.virtual_desktop_id = state.virtual_desktop_id;
                        this.detail.native_left = state.native_left;
                        this.detail.native_top = state.native_top;
                        this.detail.native_width = state.native_width;
                        this.detail.native_height = state.native_height;

                        if let Some(placement) = placement {
                            this.remember_placement(placement, primary_uuid.as_deref());
                        }
                    });
                }
            }
        })
        .detach();
    }

    /// Mirror a saved placement into the in-memory copy so the next monitor change resolves
    /// against fresh data without another round trip to the database.
    fn remember_placement(&mut self, placement: StickerPlacement, protect_uuid: Option<&str>) {
        let mut placements = placements_of(&self.detail);
        self.detail.preferred_display_uuid = Some(placement.display_uuid.clone());

        match placements
            .iter_mut()
            .find(|existing| existing.display_uuid == placement.display_uuid)
        {
            Some(existing) => *existing = placement,
            None => placements.push(placement),
        }

        let stale = crate::model::sticker::prune_placements(
            &placements,
            protect_uuid,
            crate::model::sticker::MAX_PLACEMENTS_PER_STICKER,
        );
        placements.retain(|placement| !stale.contains(&placement.display_uuid));
        self.detail.placements = placements;
    }

    fn change_color(&mut self, theme: StickerColor, cx: &mut Context<Self>) {
        self.detail.color = theme;
        self.view.set_color(cx, theme);
        cx.notify();

        let id = self.view.id(cx);
        if id <= 0 {
            return;
        }

        let store = self.store.clone();
        let events = self.sticker_events_tx.clone();
        cx.spawn(async move |entity, cx| {
            if let Err(err) = store
                .update_sticker_color(id, theme.as_str().to_string())
                .await
            {
                let _ = entity.update(cx, |this, cx| {
                    this.set_error(format!("Failed to save color: {err}"), cx);
                });
            } else {
                let _ = events.send(StickerWindowEvent::ColorChanged { id, color: theme });
            }
        })
        .detach();
    }

    fn close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }

        if self.selection_run {
            self.closing = true;
            let open_id = self.open_id;
            cx.defer(move |cx| {
                Self::try_close(open_id, cx);
            });
            return;
        }

        if !self.view.save_on_close(cx) {
            return;
        }
        self.closing = true;

        let id = self.view.id(cx);
        let original_id = self.detail.id;
        let store = self.store.clone();
        let events = self.sticker_events_tx.clone();

        cx.spawn(async move |_, cx| {
            if id > 0
                && let Err(err) = store.update_sticker_state(id, StickerState::Close).await
            {
                tracing::error!(id, error = %err, "Error saving state on close");
            }

            let _ = events.send(StickerWindowEvent::Closed { id });

            let _ = cx.update(|cx| {
                if !Self::try_close(id, cx) {
                    Self::try_close(original_id, cx);
                }
            });
        })
        .detach();
    }

    fn header_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let extension = self.view.header_extension(cx);

        h_flex()
            .absolute()
            .left_0()
            .top_0()
            .right_0()
            .items_center()
            .cursor_grab()
            .window_control_area(WindowControlArea::Drag)
            .occlude()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when_some(extension, |view, extension| view.child(extension)),
            )
            .child(self.create_button(cx))
            .child(
                Button::new("close")
                    .bg(rgba(0x000000))
                    .border_0()
                    .cursor_pointer()
                    .icon(IconName::Close)
                    .occlude()
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
            )
            .into_any_element()
    }

    fn create_sticker(&mut self, cx: &mut Context<Self>, sticker_type: &StickerType) {
        let size = match sticker_type {
            StickerType::Markdown => MarkdownSticker::default_window_size(),
            StickerType::Command => CommandSticker::default_window_size(),
            StickerType::Timer => TimerSticker::default_window_size(),
            StickerType::Paint => PaintSticker::default_window_size(),
            StickerType::File => FileSticker::default_window_size(),
        };

        let title = match sticker_type {
            StickerType::Markdown => "New Text Sticker",
            StickerType::Command => "New Command Sticker",
            StickerType::Timer => "New Timer Sticker",
            StickerType::Paint => "New Paint Sticker",
            StickerType::File => "New File Sticker",
        };

        let detail = StickerDetail {
            id: 0,
            title: title.to_string(),
            content: "".to_string(),
            color: StickerColor::Yellow,
            sticker_type: *sticker_type,
            state: StickerState::Open,
            left: 100,
            top: 100,
            width: size.width,
            height: size.height,
            top_most: false,
            created_at: 0,
            updated_at: 0,
            display_id: None,
            display_uuid: None,
            virtual_desktop_id: None,
            native_left: None,
            native_top: None,
            native_width: None,
            native_height: None,
            preferred_display_uuid: None,
            placements: Vec::new(),
        };

        let store = self.store.clone();
        let sticker_events_tx = self.sticker_events_tx.clone();
        cx.spawn(
            async move |entity, cx| match store.insert_sticker(detail).await {
                Ok(id) => {
                    let _ = sticker_events_tx.send(StickerWindowEvent::Created { id });

                    if let Err(err) =
                        StickerWindow::open_async(cx, sticker_events_tx.clone(), store.clone(), id)
                            .await
                    {
                        let _ = entity.update(cx, |this, cx| {
                            this.set_error(format!("Failed to open sticker window: {err:#}"), cx);
                        });
                    }
                }
                Err(err) => {
                    let _ = entity.update(cx, |this, cx| {
                        this.set_error(format!("Failed to create sticker: {err:#}"), cx);
                    });
                }
            },
        )
        .detach();
    }

    fn create_button(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let root_entity = cx.entity();
        Button::new("create")
            .border_0()
            .bg(rgba(0x00000000))
            .icon(IconName::Plus)
            .opacity(0.8)
            .dropdown_menu(move |menu, window, _| {
                let root_entity = root_entity.clone();
                menu.item(
                    PopupMenuItem::new("text")
                        .icon(sticker_type_icon(&StickerType::Markdown))
                        .on_click(window.listener_for(&root_entity, |this, _, _, cx| {
                            this.create_sticker(cx, &StickerType::Markdown);
                        })),
                )
                .item(
                    PopupMenuItem::new("timer")
                        .icon(sticker_type_icon(&StickerType::Timer))
                        .on_click(window.listener_for(&root_entity, |this, _, _, cx| {
                            this.create_sticker(cx, &StickerType::Timer);
                        })),
                )
                .item(
                    PopupMenuItem::new("command")
                        .icon(sticker_type_icon(&StickerType::Command))
                        .on_click(window.listener_for(&root_entity, |this, _, _, cx| {
                            this.create_sticker(cx, &StickerType::Command);
                        })),
                )
                .item(
                    PopupMenuItem::new("paint")
                        .icon(sticker_type_icon(&StickerType::Paint))
                        .on_click(window.listener_for(&root_entity, |this, _, _, cx| {
                            this.create_sticker(cx, &StickerType::Paint);
                        })),
                )
            })
            .into_any_element()
    }

    fn footer_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let extension = self.view.footer_extension(cx);
        let color_options = h_flex()
            .p_2()
            .gap_1()
            .children(StickerColor::ALL.iter().map(|&theme| {
                div()
                    .w(px(16.0))
                    .h(px(16.0))
                    .bg(theme.swatch())
                    .rounded_full()
                    .cursor_pointer()
                    .window_control_area(WindowControlArea::Drag)
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.change_color(theme, cx);
                        }),
                    )
            }));

        h_flex()
            .when(self.view.is_footer_absoute(cx), |v| {
                v.absolute().bottom_0().left_0().right_0()
            })
            .justify_end()
            .gap_2()
            .items_center()
            .occlude()
            .cursor_grab()
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when_some(extension, |view, extension| view.child(extension)),
            )
            .when(!self.view.disable_color_picker(cx), move |v| {
                v.child(color_options)
            })
            .into_any_element()
    }
}

impl Render for StickerWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.try_tick(window, cx);

        window.set_rem_size(cx.theme().font_size);

        v_flex()
            .text_color(cx.theme().foreground)
            .font_family(cx.theme().font_family.clone())
            .relative()
            .size_full()
            .on_mouse_down(MouseButton::Left, |event, window, cx| {
                if !window.is_window_active() {
                    window.activate_window();
                }
                if event.click_count >= 2 {
                    cx.stop_propagation();
                    window.prevent_default();
                }
            })
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.change_bounds(window, cx);
                }),
            )
            .when(self.view.use_default_bg(cx), |view| {
                view.bg(Rgba {
                    a: 0.85,
                    ..self.detail.color.bg()
                })
            })
            .when_some(self.error.as_ref(), |view, msg| {
                view.child(
                    div()
                        .p_2()
                        .child(Alert::error("sticker-error", msg.as_str())),
                )
            })
            .child(self.view.element())
            .when(window.is_window_active(), |view| {
                view.child(self.header_view(cx)).child(self.footer_view(cx))
            })
    }
}

fn source_title(source: &str) -> String {
    if let Ok(url) = Url::parse(source) {
        if let Some(last_segment) = url.path_segments().and_then(|segments| segments.last())
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

    if PathBuf::from(&normalized).exists() {
        return Some(normalized);
    }

    None
}

fn generate_consistence_minus_id(sources: &[String]) -> i64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    sources.hash(&mut hasher);
    let hash = hasher.finish() as i64;
    -hash.abs()
}

fn sticker_type_icon(sticker_type: &StickerType) -> IconName {
    match sticker_type {
        StickerType::Markdown => IconName::DocumentText,
        StickerType::Command => IconName::Command,
        StickerType::Timer => IconName::Bell,
        StickerType::Paint => IconName::Paint,
        StickerType::File => IconName::DocumentText,
    }
}
