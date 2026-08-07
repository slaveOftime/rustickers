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
        Foundation::{HWND, POINT, RECT},
        Graphics::Gdi::{
            GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY,
            MONITORINFO, MonitorFromPoint, MonitorFromRect,
        },
        System::Com::{CLSCTX_ALL, CoCreateInstance},
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            Shell::{IVirtualDesktopManager, VirtualDesktopManager},
            WindowsAndMessaging::{
                GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW, GetWindowRect, HWND_NOTOPMOST,
                HWND_TOPMOST, IsWindowVisible, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
                SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, USER_DEFAULT_SCREEN_DPI,
                WS_EX_TOOLWINDOW, WS_SYSMENU,
            },
        },
    },
    core::GUID,
};

use crate::model::content::{CommandContent, FileStickerContent};
use crate::model::sticker::{StickerColor, StickerDetail, StickerState, StickerType};
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
use crate::native::windows::{
    EscapeDismissTarget, StickerWindowEvent, set_escape_dismiss_target_active,
    transient_topmost::TransientTopmost,
};
use crate::storage::ArcStickerStore;

const BOUNDS_SAVE_DEBOUNCE: Duration = Duration::from_millis(200);
#[cfg(target_os = "macos")]
const NS_NORMAL_WINDOW_LEVEL: i32 = 0;

static OPEN_STICKERS: RwLock<Vec<(i64, AnyWindowHandle)>> = RwLock::new(Vec::new());
const SELECTION_RUN_OPEN_ID_MIN: i64 = i64::MAX / 2;
static NEXT_SELECTION_RUN_OPEN_ID: AtomicI64 = AtomicI64::new(i64::MAX);

/// A window rectangle in native Windows virtual-screen (physical) pixels.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
struct NativeRect {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

impl NativeRect {
    /// Convert between monitors of differing DPI, keeping the window's apparent size.
    fn scaled(self, from_scale_factor: f32, to_scale_factor: f32) -> Self {
        if from_scale_factor <= 0.0 || to_scale_factor <= 0.0 {
            return self;
        }
        let ratio = (to_scale_factor / from_scale_factor) as f64;
        Self {
            width: (self.width as f64 * ratio).round() as i32,
            height: (self.height as f64 * ratio).round() as i32,
            ..self
        }
    }
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
        let hwnd = Self::native_hwnd(window)?;
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
        Some(NativeRect {
            left: rect.left,
            top: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
        })
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
    fn primary_monitor() -> HMONITOR {
        unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) }
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

    /// Map a saved native rect onto a monitor that still exists. A rect whose monitor is gone is
    /// re-scaled from the DPI it was captured at to the primary monitor's DPI, then clamped into
    /// that monitor's work area so the sticker keeps its apparent size and stays fully visible.
    #[cfg(target_os = "windows")]
    fn visible_native_rect(rect: NativeRect, source_scale_factor: f32) -> NativeRect {
        let win_rect = RECT {
            left: rect.left,
            top: rect.top,
            right: rect.left + rect.width,
            bottom: rect.top + rect.height,
        };
        if !unsafe { MonitorFromRect(&win_rect, MONITOR_DEFAULTTONULL) }.is_invalid() {
            return rect;
        }

        let monitor = Self::primary_monitor();
        let Some(work_area) = Self::work_area(monitor) else {
            return rect;
        };
        let scaled = rect.scaled(source_scale_factor, Self::monitor_scale_factor(monitor));
        let (left, top) = clamp_into_work_area(
            (scaled.left, scaled.top),
            (scaled.width, scaled.height),
            (
                work_area.left,
                work_area.top,
                work_area.right,
                work_area.bottom,
            ),
        );
        NativeRect {
            left,
            top,
            ..scaled
        }
    }

    /// Pull a sticker back onto the primary monitor once its own monitor is unplugged. Windows
    /// leaves tool windows at their old virtual-screen coordinates, which are no longer visible.
    #[cfg(target_os = "windows")]
    fn relocate_if_off_screen(window: &Window) {
        let Some(hwnd) = Self::native_hwnd(window) else {
            return;
        };
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return;
        }
        let Some(current) = Self::native_rect(window) else {
            return;
        };
        let target = Self::visible_native_rect(current, window.scale_factor());
        if target == current {
            return;
        }

        tracing::debug!(
            ?current,
            ?target,
            "Relocating sticker from a disconnected monitor"
        );
        Self::apply_native_rect(hwnd, target);
    }

    #[cfg(not(target_os = "windows"))]
    fn relocate_if_off_screen(_window: &Window) {}

    #[cfg(target_os = "windows")]
    fn apply_native_rect(hwnd: HWND, rect: NativeRect) {
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
        }
    }

    /// Restore the exact pixel placement the sticker had when it was last saved. GPUI recreates
    /// the window using logical coordinates, which loses precision across monitors of differing
    /// DPI, so both the origin and the size are re-applied natively.
    #[cfg(target_os = "windows")]
    fn restore_native_placement(window: &Window, rect: NativeRect, source_scale_factor: f32) {
        let Some(hwnd) = Self::native_hwnd(window) else {
            return;
        };
        Self::apply_native_rect(hwnd, Self::visible_native_rect(rect, source_scale_factor));
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

        let displays = cx.displays();
        let saved_display = detail
            .display_uuid
            .as_deref()
            .and_then(|saved_uuid| {
                displays.iter().find(|display| {
                    display
                        .uuid()
                        .is_ok_and(|uuid| uuid.to_string() == saved_uuid)
                })
            })
            .or_else(|| {
                detail.display_id.and_then(|saved_id| {
                    displays
                        .iter()
                        .find(|display| u64::from(display.id()) as i64 == saved_id)
                })
            });
        let saved_monitor_missing = (detail.display_uuid.is_some() || detail.display_id.is_some())
            && saved_display.is_none();
        let display_id = saved_display
            .map(|display| display.id())
            .or_else(|| cx.primary_display().map(|display| display.id()));
        let bounds = if saved_monitor_missing {
            let display_bounds = cx
                .primary_display()
                .map(|display| display.visible_bounds())
                .unwrap_or(bounds);
            Bounds::new(display_bounds.origin, bounds.size)
        } else {
            bounds
        };

        let top_most = detail.top_most;
        let transient_topmost = top_most && (selection_run || detail.id <= 0);
        #[cfg(target_os = "windows")]
        let virtual_desktop_id = detail.virtual_desktop_id.clone();
        #[cfg(target_os = "windows")]
        let restore_rect = NativeRect {
            left: detail.native_left.unwrap_or(bounds.left().to_f64() as i32),
            top: detail.native_top.unwrap_or(bounds.top().to_f64() as i32),
            width: detail
                .native_width
                .unwrap_or(bounds.size.width.to_f64() as i32),
            height: detail
                .native_height
                .unwrap_or(bounds.size.height.to_f64() as i32),
        };
        // The saved native rect was captured at the saved monitor's DPI. Derive that scale from
        // the logical size stored alongside it so an unplugged monitor can still be compensated.
        #[cfg(target_os = "windows")]
        let restore_scale_factor = match (detail.native_width, detail.width) {
            (Some(native_width), logical_width) if logical_width > 0 && native_width > 0 => {
                native_width as f32 / logical_width as f32
            }
            _ => 1.0,
        };

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
                    StickerWindow::restore_native_placement(
                        window,
                        restore_rect,
                        restore_scale_factor,
                    );
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
                    if bounds_entity
                        .update_in(cx, |this, window, cx| {
                            this.tick_bounds_state(window, cx);
                        })
                        .is_err()
                    {
                        break;
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

        Self {
            open_id,
            store,
            detail,
            sticker_events_tx,
            view,
            last_bounds: None,
            last_bounds_change_at: None,
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

    fn tick_bounds_state(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.view.id(cx) <= 0 {
            return;
        }

        Self::relocate_if_off_screen(window);

        let current = self.current_bounds(window, cx);
        let changed = self
            .last_bounds
            .as_ref()
            .map(|x| x != &current)
            .unwrap_or(true);

        if changed {
            self.last_bounds = Some(current);
            self.last_bounds_change_at = Some(Instant::now());
            return;
        }

        if self
            .last_bounds_change_at
            .is_some_and(|changed_at| changed_at.elapsed() >= BOUNDS_SAVE_DEBOUNCE)
        {
            self.last_bounds_change_at = None;
            self.change_bounds(window, cx);
        }
    }

    fn try_tick(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.last_bounds.is_none() {
            self.last_bounds = Some(self.current_bounds(window, cx));
        }
        self.tick_bounds_state(window, cx);
    }

    fn current_bounds(&self, window: &Window, cx: &Context<Self>) -> WindowState {
        let bounds = window.bounds();
        let display = window.display(cx);
        let display_id = display.as_ref().map(|x| u64::from(x.id()) as i64);
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
        let bounds = self.current_bounds(window, cx);
        if bounds.left != self.detail.left
            || bounds.top != self.detail.top
            || bounds.width != self.detail.width
            || bounds.height != self.detail.height
            || bounds.display_id != self.detail.display_id
            || bounds.display_uuid != self.detail.display_uuid
            || bounds.virtual_desktop_id != self.detail.virtual_desktop_id
            || bounds.native_left != self.detail.native_left
            || bounds.native_top != self.detail.native_top
            || bounds.native_width != self.detail.native_width
            || bounds.native_height != self.detail.native_height
        {
            self.last_bounds = Some(bounds.clone());

            let id = self.view.id(cx);
            let store = self.store.clone();

            tracing::debug!("Save bounds state: {:?}", &bounds);

            cx.spawn(async move |this, cx| {
                if let Err(err) = store
                    .update_sticker_bounds(
                        id,
                        bounds.left,
                        bounds.top,
                        bounds.width,
                        bounds.height,
                        bounds.display_id,
                        bounds.display_uuid.clone(),
                        bounds.virtual_desktop_id.clone(),
                        bounds.native_left,
                        bounds.native_top,
                        bounds.native_width,
                        bounds.native_height,
                    )
                    .await
                {
                    let _ = this.update(cx, |this, cx| {
                        this.set_error(format!("Failed to save window bounds: {err}"), cx);
                    });
                } else {
                    let _ = this.update(cx, |this, _| {
                        this.detail.left = bounds.left;
                        this.detail.top = bounds.top;
                        this.detail.width = bounds.width;
                        this.detail.height = bounds.height;
                        this.detail.display_id = bounds.display_id;
                        this.detail.display_uuid = bounds.display_uuid;
                        this.detail.virtual_desktop_id = bounds.virtual_desktop_id;
                        this.detail.native_left = bounds.native_left;
                        this.detail.native_top = bounds.native_top;
                        this.detail.native_width = bounds.native_width;
                        this.detail.native_height = bounds.native_height;
                    });
                }
            })
            .detach();
        }
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

/// Move a window rect back inside a work area, keeping it fully visible when it fits.
fn clamp_into_work_area(
    position: (i32, i32),
    size: (i32, i32),
    work_area: (i32, i32, i32, i32),
) -> (i32, i32) {
    let (left, top) = position;
    let (width, height) = size;
    let (area_left, area_top, area_right, area_bottom) = work_area;
    let max_left = (area_right - width).max(area_left);
    let max_top = (area_bottom - height).max(area_top);
    (
        left.clamp(area_left, max_left),
        top.clamp(area_top, max_top),
    )
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

#[cfg(test)]
mod tests {
    use super::{NativeRect, clamp_into_work_area};

    const PRIMARY: (i32, i32, i32, i32) = (0, 0, 1920, 1040);

    #[test]
    fn position_on_an_unplugged_right_monitor_moves_to_the_primary_edge() {
        assert_eq!(
            clamp_into_work_area((2307, 300), (210, 114), PRIMARY),
            (1710, 300)
        );
    }

    #[test]
    fn position_above_the_primary_monitor_is_pulled_down() {
        assert_eq!(
            clamp_into_work_area((400, -1217), (210, 114), PRIMARY),
            (400, 0)
        );
    }

    #[test]
    fn windows_larger_than_the_work_area_align_to_its_origin() {
        assert_eq!(
            clamp_into_work_area((2307, -1217), (3000, 2000), PRIMARY),
            (0, 0)
        );
    }

    #[test]
    fn moving_to_a_higher_dpi_monitor_grows_the_pixel_size() {
        let rect = NativeRect {
            left: 100,
            top: 200,
            width: 210,
            height: 114,
        };
        assert_eq!(
            rect.scaled(1.0, 1.5),
            NativeRect {
                left: 100,
                top: 200,
                width: 315,
                height: 171,
            }
        );
    }

    #[test]
    fn moving_to_a_lower_dpi_monitor_shrinks_the_pixel_size() {
        let rect = NativeRect {
            left: 0,
            top: 0,
            width: 315,
            height: 171,
        };
        assert_eq!(
            rect.scaled(1.5, 1.0),
            NativeRect {
                left: 0,
                top: 0,
                width: 210,
                height: 114,
            }
        );
    }

    #[test]
    fn equal_dpi_monitors_keep_the_pixel_size() {
        let rect = NativeRect {
            left: 5,
            top: 6,
            width: 210,
            height: 114,
        };
        assert_eq!(rect.scaled(1.25, 1.25), rect);
    }

    #[test]
    fn invalid_scale_factors_leave_the_rect_untouched() {
        let rect = NativeRect {
            left: 5,
            top: 6,
            width: 210,
            height: 114,
        };
        assert_eq!(rect.scaled(0.0, 1.5), rect);
        assert_eq!(rect.scaled(1.5, 0.0), rect);
    }
}
