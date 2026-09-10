//! Windows workaround for a crash in gpui's system-wake handling.
//!
//! gpui registers for `WM_POWERBROADCAST` (resume from sleep) on its hidden
//! message-only platform window and invokes the wake callback synchronously.
//! Because the message is *sent*, Windows can deliver it reentrantly from a
//! nested message pump (e.g. inside a COM call) while gpui's `App` is mutably
//! borrowed, and gpui's `borrow_mut()` then panics — fatal because the
//! release profile sets `panic = "abort"`.
//!
//! Nothing in this app observes system-wake events, so we subclass the
//! platform window and swallow `WM_POWERBROADCAST` before gpui's window
//! procedure can see it.

#[cfg(target_os = "windows")]
mod imp {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, WM_POWERBROADCAST};
    use windows::core::{PCWSTR, w};

    /// Parent handle under which gpui creates its hidden message-only window.
    const HWND_MESSAGE: HWND = HWND(-3_isize as *mut core::ffi::c_void);
    const GPUI_PLATFORM_WINDOW_CLASS: PCWSTR = w!("Zed::PlatformWindow");
    const SUBCLASS_ID: usize = 0x5253_5449; // "RSTI"

    unsafe extern "system" fn swallow_power_broadcast(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _subclass_id: usize,
        _ref_data: usize,
    ) -> LRESULT {
        if msg == WM_POWERBROADCAST {
            // Return TRUE, matching what gpui's own handler returns.
            return LRESULT(1);
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    pub(crate) fn install() {
        let hwnd = unsafe {
            FindWindowExW(
                Some(HWND_MESSAGE),
                None,
                GPUI_PLATFORM_WINDOW_CLASS,
                PCWSTR::null(),
            )
        };
        let hwnd = match hwnd {
            Ok(hwnd) if hwnd != HWND::default() => hwnd,
            _ => {
                tracing::warn!("gpui platform window not found; system-wake crash guard not installed");
                return;
            }
        };
        unsafe {
            let _ = SetWindowSubclass(hwnd, Some(swallow_power_broadcast), SUBCLASS_ID, 0);
        }
        tracing::info!("Installed system-wake crash guard on gpui platform window");
    }
}

#[cfg(target_os = "windows")]
pub(crate) use imp::install;

#[cfg(not(target_os = "windows"))]
pub(crate) fn install() {}
