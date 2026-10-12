//! Fail-closed destination restoration and paste dispatch.
use anyhow::{Context, bail};
use rdev::{EventType, Key, simulate};
#[cfg(target_os = "windows")]
use std::{
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "windows")]
mod platform {
    use anyhow::{Result, bail};
    use std::{
        thread,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
        },
        WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow, SW_RESTORE,
            SetForegroundWindow, ShowWindow,
        },
    };
    pub fn verify(handle: u64) -> Result<()> {
        let hwnd = handle as *mut core::ffi::c_void;
        unsafe {
            let mut pid = 0;
            if IsWindow(hwnd) == 0 || GetWindowThreadProcessId(hwnd, &mut pid) == 0 {
                bail!("The original window has closed. Use Copy only.");
            }
            // Cross-process destinations must actually be foreground before we
            // synthesise Ctrl+V; otherwise the simulated paste land in the
            // wrong app. Same-process targets skip this check: the picker was
            // opened from one of them, the receiving control is part of this
            // app, and Windows is more lenient about foreground transitions
            // within a single process. We still gate on `IsWindow` above so a
            // closed same-process window doesn't get a phantom paste.
            let same_process = pid == std::process::id();
            if !same_process && GetForegroundWindow() != hwnd {
                bail!("The original destination is not focused. Use Copy only.");
            }
        }
        Ok(())
    }
    pub fn foreground(handle: u64) -> Result<()> {
        let hwnd = handle as *mut core::ffi::c_void;
        let same_process = unsafe {
            let mut pid = 0;
            if IsWindow(hwnd) == 0 || GetWindowThreadProcessId(hwnd, &mut pid) == 0 {
                bail!("The original window has closed. Use Copy only.");
            }
            // The self-pid gate used to bail out here, which was the bug: the
            // rustickers process is also the source of every native input box
            // we paste into (selection popup, sticker MD editor, settings
            // fields), so refusing same-process activations blocked the TXT
            // picker's paste mode from supplying any of them.
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd);
            pid == std::process::id()
        };
        // Same-process foreground switches are unconditionally allowed by
        // Windows even after the picker has been destroyed; the receiving
        // child control regains focus when its parent window is recomposed,
        // so there is no observable drift to wait for.
        if same_process {
            return Ok(());
        }
        // USER32 sometimes applies the actual foreground change a few
        // milliseconds after SetForegroundWindow returns. Poll briefly so
        // callers can rely on `GetForegroundWindow() == handle` once the
        // function returns Ok, while still bailing out fast if the call
        // genuinely was blocked by the foreground lock (which the call site
        // minimises by always invoking foreground() while still inside the
        // foreground process).
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            unsafe {
                if GetForegroundWindow() == hwnd {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                bail!("The original destination is not focused. Use Copy only.");
            }
            thread::sleep(Duration::from_millis(15));
        }
    }
    pub fn modifiers_released() -> bool {
        [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
            .into_iter()
            .all(|key| unsafe { GetAsyncKeyState(key as i32) >= 0 })
    }
}

pub fn foreground(handle: Option<u64>) -> anyhow::Result<()> {
    let handle = handle
        .filter(|h| *h != 0)
        .context("No original destination is available. Use Copy only.")?;
    #[cfg(target_os = "windows")]
    {
        platform::foreground(handle)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = handle;
        bail!("Verified paste is unavailable on this platform. Use Copy only.")
    }
}

/// Never synthesize releases for modifiers held by the user.
pub fn wait_for_modifiers() -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !platform::modifiers_released() {
            if Instant::now() >= deadline {
                bail!("Release shortcut modifiers and retry, or use Copy only.");
            }
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        bail!("Verified paste is unavailable on this platform. Use Copy only.")
    }
}

/// Always release our synthetic keys, including after a failed key-down.
fn dispatch_paste(mut send: impl FnMut(EventType) -> anyhow::Result<()>) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    let primary = Key::MetaLeft;
    #[cfg(not(target_os = "macos"))]
    let primary = Key::ControlLeft;
    let press =
        send(EventType::KeyPress(primary)).and_then(|_| send(EventType::KeyPress(Key::KeyV)));
    let release_v = send(EventType::KeyRelease(Key::KeyV));
    let release_primary = send(EventType::KeyRelease(primary));
    press.and(release_v).and(release_primary)
}

pub fn paste_foreground(handle: Option<u64>) -> anyhow::Result<()> {
    let handle = handle.context("No destination. Use Copy only.")?;
    #[cfg(target_os = "windows")]
    {
        platform::verify(handle)?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = handle;
        bail!("Verified paste is unavailable on this platform. Use Copy only.");
    }
    #[allow(unreachable_code)]
    dispatch_paste(|event| {
        simulate(&event).map_err(|_| anyhow::anyhow!("Paste input dispatch failed; use Copy only."))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_destination_is_an_error() {
        assert!(foreground(None).is_err());
    }
    #[test]
    fn failed_press_still_releases_both_keys() {
        let mut events = Vec::new();
        let result = dispatch_paste(|event| {
            events.push(event);
            if events.len() == 2 {
                bail!("injected input failure");
            }
            Ok(())
        });
        assert!(result.is_err());
        assert!(matches!(events[2], EventType::KeyRelease(Key::KeyV)));
        assert!(matches!(events[3], EventType::KeyRelease(_)));
    }
    #[test]
    fn dispatch_reports_release_failure() {
        let mut n = 0;
        assert!(
            dispatch_paste(|_| {
                n += 1;
                if n == 3 {
                    bail!("release failed");
                }
                Ok(())
            })
            .is_err()
        );
        assert_eq!(n, 4);
    }
}
