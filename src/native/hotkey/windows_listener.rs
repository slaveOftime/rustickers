//! Own the hook and session-notification window on the keyboard pump thread.
//!
//! Lock/disconnect breaks the input stream and can strand held modifiers.
//! Reinstall on return as well as resetting state: Windows can silently remove
//! a timed-out low-level hook, and its old handle is not a liveness check.
//! See https://learn.microsoft.com/windows/win32/winmsg/lowlevelkeyboardproc.
use super::WINDOWS_CONTEXT;
use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND},
    System::{
        LibraryLoader::GetModuleHandleW,
        RemoteDesktop::{
            NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification,
            WTSUnRegisterSessionNotification,
        },
    },
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HHOOK, HOOKPROC,
        KillTimer, MSG, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND, PostMessageW,
        RegisterClassW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, UnregisterClassW,
        WH_KEYBOARD_LL, WM_APP, WM_POWERBROADCAST, WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSW,
        WTS_CONSOLE_CONNECT, WTS_CONSOLE_DISCONNECT, WTS_REMOTE_CONNECT, WTS_REMOTE_DISCONNECT,
        WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
    },
};

const CLASS: *const u16 = windows_sys::core::w!("RustickersHotkeyListener");
const SESSION_CHANGE: u32 = WM_APP + 1;
const POWER_CHANGE: u32 = WM_APP + 2;
const RETRY_TIMER: usize = 1;

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    // Session/power notifications can be sent reentrantly. Defer all hook
    // mutation to the outer pump, where no callback holds the context lock.
    let deferred = match message {
        WM_WTSSESSION_CHANGE => Some(SESSION_CHANGE),
        WM_POWERBROADCAST => Some(POWER_CHANGE),
        _ => None,
    };
    if let Some(deferred) = deferred {
        unsafe { PostMessageW(hwnd, deferred, wparam, lparam) };
        return 1;
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

pub(super) struct Listener {
    window: HWND,
    instance: HINSTANCE,
    hook: HHOOK,
    hook_proc: HOOKPROC,
    registered: bool,
    retry_pending: bool,
}
impl Listener {
    pub(super) fn new() -> anyhow::Result<Self> {
        Self::with_hook(Some(super::keyboard_hook))
    }

    fn with_hook(hook_proc: HOOKPROC) -> anyhow::Result<Self> {
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        if instance.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: CLASS,
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut listener = Self {
            window: std::ptr::null_mut(),
            instance,
            hook: std::ptr::null_mut(),
            hook_proc,
            registered: false,
            retry_pending: false,
        };
        // Hidden top-level window: unlike HWND_MESSAGE, it also receives power
        // broadcasts. It never appears in the taskbar or takes focus.
        listener.window = unsafe {
            CreateWindowExW(
                0,
                CLASS,
                CLASS,
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        if listener.window.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        if unsafe { WTSRegisterSessionNotification(listener.window, NOTIFY_FOR_THIS_SESSION) } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        listener.registered = true;
        listener.install_hook()?;
        tracing::info!("Global keyboard hook installed; monitoring session transitions");
        Ok(listener)
    }

    fn install_hook(&mut self) -> std::io::Result<()> {
        self.hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, self.hook_proc, self.instance, 0) };
        if self.hook.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        unsafe { KillTimer(self.window, RETRY_TIMER) };
        self.retry_pending = false;
        Ok(())
    }

    fn interrupt(&mut self) {
        if !self.window.is_null() {
            unsafe { KillTimer(self.window, RETRY_TIMER) };
        }
        self.retry_pending = false;
        if !self.hook.is_null() {
            // A timed-out hook may already have been silently removed. Ignore
            // that unhook error, but never retain its stale handle.
            unsafe { UnhookWindowsHookEx(self.hook) };
            self.hook = std::ptr::null_mut();
        }
        if let Some(context) = WINDOWS_CONTEXT.get() {
            context
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .matcher
                .reset();
        }
    }

    fn recover(&mut self, reason: usize) {
        self.interrupt();
        match self.install_hook() {
            Ok(()) => tracing::info!(
                reason,
                "Global keyboard hook reinstalled after session transition"
            ),
            Err(error) => {
                tracing::warn!(%error, reason, "Could not reinstall keyboard hook; retrying");
                self.retry_pending = unsafe { SetTimer(self.window, RETRY_TIMER, 1000, None) } != 0;
                if !self.retry_pending {
                    tracing::error!(error = %std::io::Error::last_os_error(), "Could not schedule keyboard hook retry");
                }
            }
        }
    }

    pub(super) fn process(&mut self, message: &MSG) {
        if message.hwnd == self.window {
            match (message.message, message.wParam as u32) {
                (
                    SESSION_CHANGE,
                    WTS_SESSION_LOCK | WTS_CONSOLE_DISCONNECT | WTS_REMOTE_DISCONNECT,
                )
                | (POWER_CHANGE, PBT_APMSUSPEND) => {
                    self.interrupt();
                    tracing::info!(
                        reason = message.wParam,
                        "Global keyboard hook paused for session transition"
                    );
                    return;
                }
                (SESSION_CHANGE, WTS_SESSION_UNLOCK | WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT)
                | (POWER_CHANGE, PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND) => {
                    self.recover(message.wParam);
                    return;
                }
                (WM_TIMER, _) if message.wParam == RETRY_TIMER => {
                    // A killed timer may still have a queued message.
                    if self.retry_pending {
                        self.recover(message.wParam);
                    }
                    return;
                }
                _ => {}
            }
        }
        unsafe { DispatchMessageW(message) };
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        self.interrupt();
        if !self.window.is_null() {
            unsafe {
                if self.registered {
                    WTSUnRegisterSessionNotification(self.window);
                }
                DestroyWindow(self.window);
            }
        }
        unsafe { UnregisterClassW(CLASS, self.instance) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_F24,
        },
        WindowsAndMessaging::{
            CallNextHookEx, KBDLLHOOKSTRUCT, PM_REMOVE, PeekMessageW, SendMessageW, WM_KEYDOWN,
        },
    };

    const MARKER: usize = 0x5253_5448;
    static PROBE: AtomicUsize = AtomicUsize::new(0);
    static GUARD: AtomicUsize = AtomicUsize::new(0);

    unsafe fn probe(code: i32, wparam: usize, lparam: isize, counter: &AtomicUsize) -> isize {
        if code >= 0 {
            let event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            if event.vkCode == u32::from(VK_F24) && event.dwExtraInfo == MARKER {
                if wparam as u32 == WM_KEYDOWN {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                // Never deliver the test key to the user's foreground app.
                return 1;
            }
        }
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
    }
    unsafe extern "system" fn probe_hook(code: i32, wparam: usize, lparam: isize) -> isize {
        unsafe { probe(code, wparam, lparam, &PROBE) }
    }
    unsafe extern "system" fn guard_hook(code: i32, wparam: usize, lparam: isize) -> isize {
        unsafe { probe(code, wparam, lparam, &GUARD) }
    }
    struct Guard(HHOOK);
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe { UnhookWindowsHookEx(self.0) };
        }
    }

    fn pump_until(listener: &mut Listener, predicate: impl Fn(&Listener) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mut message = MSG::default();
            while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0
            {
                listener.process(&message);
            }
            if predicate(listener) {
                return;
            }
            assert!(Instant::now() < deadline, "Windows hook probe timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn inject_probe(listener: &mut Listener, expected_probe: usize, expected_guard: usize) {
        let injector = std::thread::spawn(|| {
            let inputs = [0, KEYEVENTF_KEYUP].map(|flags| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_F24,
                        dwFlags: flags,
                        dwExtraInfo: MARKER,
                        ..Default::default()
                    },
                },
            });
            assert_eq!(
                unsafe {
                    SendInput(
                        inputs.len() as u32,
                        inputs.as_ptr(),
                        std::mem::size_of::<INPUT>() as i32,
                    )
                },
                inputs.len() as u32
            );
        });
        pump_until(listener, |_| {
            injector.is_finished()
                && PROBE.load(Ordering::SeqCst) + GUARD.load(Ordering::SeqCst)
                    >= expected_probe + expected_guard
        });
        injector.join().unwrap();
        assert_eq!(PROBE.load(Ordering::SeqCst), expected_probe);
        assert_eq!(GUARD.load(Ordering::SeqCst), expected_guard);
    }

    #[test]
    #[ignore = "opt-in Windows desktop test: installs temporary hooks and consumes tagged F24 input"]
    fn session_notifications_restore_native_hook_delivery() {
        // No app windows, clipboard access, or real lock/unlock. A second hook
        // consumes the tagged probe even while the listener's hook is absent.
        PROBE.store(0, Ordering::SeqCst);
        GUARD.store(0, Ordering::SeqCst);
        let guard = Guard(unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(guard_hook),
                GetModuleHandleW(std::ptr::null()),
                0,
            )
        });
        assert!(!guard.0.is_null());
        let mut listener = Listener::with_hook(Some(probe_hook)).unwrap();
        inject_probe(&mut listener, 1, 0);

        // Simulate silent hook removal: the owner still has a stale handle.
        assert_ne!(unsafe { UnhookWindowsHookEx(listener.hook) }, 0);
        inject_probe(&mut listener, 1, 1);
        unsafe {
            SendMessageW(
                listener.window,
                WM_WTSSESSION_CHANGE,
                WTS_SESSION_UNLOCK as usize,
                0,
            )
        };
        pump_until(&mut listener, |_| true);
        assert!(!listener.hook.is_null());
        inject_probe(&mut listener, 2, 1);

        for cycle in 0..3 {
            unsafe {
                SendMessageW(
                    listener.window,
                    WM_WTSSESSION_CHANGE,
                    WTS_SESSION_LOCK as usize,
                    0,
                )
            };
            pump_until(&mut listener, |l| l.hook.is_null());
            inject_probe(&mut listener, 2 + cycle, 2 + cycle);
            unsafe {
                SendMessageW(
                    listener.window,
                    WM_WTSSESSION_CHANGE,
                    WTS_SESSION_UNLOCK as usize,
                    0,
                )
            };
            pump_until(&mut listener, |l| !l.hook.is_null());
            inject_probe(&mut listener, 3 + cycle, 2 + cycle);
        }
        // Force one reinstall failure, then let the real timer retry it.
        listener.hook_proc = None;
        listener.recover(WTS_SESSION_UNLOCK as usize);
        assert!(listener.hook.is_null());
        assert!(listener.retry_pending);
        listener.hook_proc = Some(probe_hook);
        pump_until(&mut listener, |l| !l.hook.is_null());
        assert!(!listener.retry_pending);
        inject_probe(&mut listener, 6, 4);

        // A stale queued retry must not reinstall a hook after a new lock.
        listener.hook_proc = None;
        listener.recover(WTS_SESSION_UNLOCK as usize);
        assert!(listener.retry_pending);
        listener.hook_proc = Some(probe_hook);
        unsafe {
            SendMessageW(
                listener.window,
                WM_WTSSESSION_CHANGE,
                WTS_SESSION_LOCK as usize,
                0,
            )
        };
        pump_until(&mut listener, |l| !l.retry_pending);
        listener.process(&MSG {
            hwnd: listener.window,
            message: WM_TIMER,
            wParam: RETRY_TIMER,
            ..Default::default()
        });
        assert!(listener.hook.is_null());
        unsafe {
            SendMessageW(
                listener.window,
                WM_WTSSESSION_CHANGE,
                WTS_SESSION_UNLOCK as usize,
                0,
            )
        };
        pump_until(&mut listener, |l| !l.hook.is_null());
        inject_probe(&mut listener, 7, 4);
        println!(
            "Native hook delivery recovered after stale-hook removal, 3 notification cycles, and retry failure"
        );
    }
}
