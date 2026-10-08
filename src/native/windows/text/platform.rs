//! Native presentation for the short-lived TXTs utility window.
use gpui::Window;

pub(super) fn raise(window: &mut Window) {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos,
        };
        if let Ok(handle) = HasWindowHandle::window_handle(window)
            && let RawWindowHandle::Win32(handle) = handle.as_raw()
        {
            // Floating is not sufficient when another app already owns topmost windows.
            unsafe {
                SetWindowPos(
                    handle.hwnd.get() as _,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                );
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        use cocoa::{
            appkit::{NSApplication, NSMainMenuWindowLevel, NSWindow},
            base::{YES, nil},
        };
        use objc::{msg_send, sel, sel_impl};
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = HasWindowHandle::window_handle(window)
            && let RawWindowHandle::AppKit(handle) = handle.as_raw()
        {
            unsafe {
                let view = handle.ns_view.as_ptr() as cocoa::base::id;
                let native: cocoa::base::id = msg_send![view, window];
                if !native.is_null() {
                    native.setLevel_((NSMainMenuWindowLevel + 1) as _);
                    NSApplication::sharedApplication(nil).activateIgnoringOtherApps_(YES);
                    native.makeKeyAndOrderFront_(nil);
                }
            }
        }
    }
    window.activate_window();
}
