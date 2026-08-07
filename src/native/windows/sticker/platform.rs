//! The native window plumbing behind sticker windows.
//!
//! Every call that needs a platform window handle lives here, so the rest of the sticker module
//! reads as plain cross-platform code. Functions that only make sense on one platform are no-ops
//! (or return `None`) everywhere else rather than being conditionally compiled at their call
//! sites.

use gpui::{App, PlatformDisplay, Window};

use crate::native::windows::placement::{DisplayEntry, NativeRect};

#[cfg(target_os = "macos")]
use cocoa::{
    appkit::{
        NSApplication, NSMainMenuWindowLevel, NSWindow, NSWindowButton, NSWindowCollectionBehavior,
        NSWindowStyleMask,
    },
    base::{YES, nil},
};
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

#[cfg(target_os = "windows")]
use crate::native::windows::placement::rect_settled;

/// How many times a placement is re-applied before giving up. Crossing a DPI boundary costs one
/// extra pass, see [`rect_settled`] for why.
#[cfg(target_os = "windows")]
const MAX_PLACEMENT_PASSES: usize = 3;

#[cfg(target_os = "macos")]
const NS_NORMAL_WINDOW_LEVEL: i32 = 0;

/// A stable key for a monitor. GPUI derives the UUID from the display device name; the raw handle
/// is only a last resort, it changes between sessions.
pub(super) fn display_uuid(display: &dyn PlatformDisplay) -> String {
    match display.uuid() {
        Ok(uuid) => uuid.to_string(),
        Err(err) => {
            let raw = u64::from(display.id());
            tracing::warn!(raw, error = ?err, "Monitor has no stable UUID, falling back to its handle");
            format!("id:{raw}")
        }
    }
}

/// The monitors that are connected right now, described the way the placement resolver needs
/// them: stable UUID, DPI scale and work area in native pixels.
#[cfg(target_os = "windows")]
pub(super) fn display_snapshot(cx: &App) -> Vec<DisplayEntry> {
    let primary = cx.primary_display().map(|display| u64::from(display.id()));
    cx.displays()
        .into_iter()
        .filter_map(|display| {
            let raw = u64::from(display.id());
            let monitor = HMONITOR(raw as _);
            let area = work_area(monitor)?;
            Some(DisplayEntry {
                uuid: display_uuid(display.as_ref()),
                display_id: Some(raw as i64),
                scale_factor: monitor_scale_factor(monitor),
                work_area: (area.left, area.top, area.right, area.bottom),
                is_primary: primary == Some(raw),
            })
        })
        .collect()
}

#[cfg(not(target_os = "windows"))]
pub(super) fn display_snapshot(_cx: &App) -> Vec<DisplayEntry> {
    Vec::new()
}

/// Apply the window styles a sticker needs, right after it is created and whenever its
/// always-on-top behaviour has to be re-asserted.
pub(super) fn configure_window(window: &Window, top_most: bool) {
    #[cfg(target_os = "macos")]
    configure_appkit_window(window, top_most);
    #[cfg(target_os = "windows")]
    configure_win32_window(window, top_most);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (window, top_most);
    }
}

/// Bring a sticker to the front from a context where the application may not be active.
///
/// Only macOS needs this: AppKit can defer presenting a new window of an inactive application,
/// and re-running the window setup performs the activation dance that fixes it.
pub(super) fn refocus_window(window: &Window, top_most: bool) {
    #[cfg(target_os = "macos")]
    configure_appkit_window(window, top_most);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, top_most);
    }
}

#[cfg(target_os = "windows")]
pub(super) fn current_virtual_desktop_id(window: &Window) -> Option<String> {
    let hwnd = hwnd(window)?;
    let manager = virtual_desktop_manager().ok()?;
    unsafe { manager.GetWindowDesktopId(hwnd) }
        .ok()
        .map(|id| format!("{id:?}"))
}

#[cfg(not(target_os = "windows"))]
pub(super) fn current_virtual_desktop_id(_window: &Window) -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
pub(super) fn restore_virtual_desktop(window: &Window, desktop_id: Option<&str>) {
    let Some(desktop_id) = desktop_id.and_then(|id| GUID::try_from(id).ok()) else {
        return;
    };
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    let result = virtual_desktop_manager()
        .and_then(|manager| unsafe { manager.MoveWindowToDesktop(hwnd, &desktop_id) });
    if let Err(err) = result {
        tracing::warn!(error = ?err, "Failed to restore sticker virtual desktop");
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn restore_virtual_desktop(_window: &Window, _desktop_id: Option<&str>) {}

/// The window's outer rectangle in native pixels.
#[cfg(target_os = "windows")]
pub(super) fn window_rect(window: &Window) -> Option<NativeRect> {
    hwnd_rect(hwnd(window)?)
}

#[cfg(not(target_os = "windows"))]
pub(super) fn window_rect(_window: &Window) -> Option<NativeRect> {
    None
}

#[cfg(target_os = "windows")]
pub(super) fn window_is_visible(window: &Window) -> bool {
    hwnd(window).is_some_and(|hwnd| unsafe { IsWindowVisible(hwnd) }.as_bool())
}

#[cfg(not(target_os = "windows"))]
pub(super) fn window_is_visible(_window: &Window) -> bool {
    true
}

/// Put the window exactly where it is asked to go, in native pixels and even across a DPI
/// boundary. GPUI recreates windows from logical coordinates, which cannot express a precise
/// placement across monitors of differing DPI.
#[cfg(target_os = "windows")]
pub(super) fn apply_window_rect(window: &Window, rect: NativeRect) {
    if let Some(hwnd) = hwnd(window) {
        apply_rect(hwnd, rect);
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn apply_window_rect(_window: &Window, _rect: NativeRect) {}

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
pub(super) struct ScaleResync {
    hwnd: isize,
    dpi: u32,
    rect: NativeRect,
}

impl ScaleResync {
    #[cfg(target_os = "windows")]
    pub(super) fn apply(self) {
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
        apply_rect(hwnd, self.rect);
    }

    #[cfg(not(target_os = "windows"))]
    pub(super) fn apply(self) {}
}

/// Tell GPUI about the DPI of the monitor the sticker now lives on.
///
/// GPUI caches a window's scale factor and only refreshes it from `WM_DPICHANGED`. Windows does
/// not always raise that message when a monitor disappears and the window is herded onto another
/// one, which leaves GPUI laying the sticker out for the DPI of a monitor it left: the window has
/// the right size and position but its contents are drawn at the wrong scale. The returned
/// correction re-applies the very rectangle passed to it, so it does not fight the placement.
#[cfg(target_os = "windows")]
pub(super) fn scale_resync(window: &Window) -> Option<ScaleResync> {
    let hwnd = hwnd(window)?;
    let rect = hwnd_rect(hwnd)?;
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let scale_factor = monitor_scale_factor(monitor);
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
pub(super) fn scale_resync(_window: &Window) -> Option<ScaleResync> {
    None
}

#[cfg(target_os = "windows")]
fn hwnd(window: &Window) -> Option<HWND> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(HWND(handle.hwnd.get() as *mut _))
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

/// Move and resize a window, retrying until it really lands where it was asked to.
///
/// GPUI creates every window on the primary monitor, so restoring a sticker that lives elsewhere
/// means moving it between monitors. When their DPI differs Windows raises `WM_DPICHANGED` while
/// this very `SetWindowPos` is running and GPUI answers it by applying the rectangle the system
/// suggests, which is our size multiplied by the DPI ratio. Applying the same rectangle again
/// fixes it: by then the window already sits on the target monitor, so no further DPI change is
/// raised.
#[cfg(target_os = "windows")]
fn apply_rect(hwnd: HWND, rect: NativeRect) {
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

        let Some(actual) = hwnd_rect(hwnd) else {
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

#[cfg(target_os = "windows")]
fn virtual_desktop_manager() -> windows::core::Result<IVirtualDesktopManager> {
    unsafe { CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_ALL) }
}

/// Windows excludes tool windows from Win+D's minimize-all operation. Preserve all existing
/// extended styles because GPUI also uses them for rendering and activation.
#[cfg(target_os = "windows")]
fn configure_win32_window(window: &Window, top_most: bool) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };

    unsafe {
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

/// `Stationary` opts sticker windows out of macOS's Show Desktop animation, so clicking the
/// desktop does not push them toward the screen edges.
#[cfg(target_os = "macos")]
fn configure_appkit_window(window: &Window, top_most: bool) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };

    unsafe {
        let view = handle.ns_view.as_ptr() as cocoa::base::id;
        let native_window: cocoa::base::id = msg_send![view, window];
        if native_window.is_null() {
            return;
        }

        // GPUI currently omits NSResizableWindowMask when titlebar is None, even when
        // WindowOptions::is_resizable is true. Sticker windows are borderless, so restore the
        // native resize style explicitly.
        let style = native_window.styleMask();
        native_window.setStyleMask_(style | NSWindowStyleMask::NSResizableWindowMask);

        // Adding the native resize style can make AppKit recreate the standard titlebar controls.
        // Keep native edge resizing but hide the traffic-light buttons.
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

        let transient_behavior =
            NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
                | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary;
        let mut behavior = native_window.collectionBehavior()
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary;
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
            // GPUI's floating level is only slightly above normal windows. Use the status-window
            // level and explicitly order the sticker to the front so it appears on the active
            // Space without activating the whole application. The global hotkey fires while
            // Finder (or another application) is active, and AppKit may defer presentation of a
            // newly created Metal window belonging to an inactive application until that
            // application receives an event. Activate Rustickers first, then make this window the
            // key/front window.
            let app = NSApplication::sharedApplication(nil);
            app.activateIgnoringOtherApps_(YES);
            let _: () = msg_send![native_window, makeKeyAndOrderFront: nil];
            native_window.orderFrontRegardless();
        }
    }
}
