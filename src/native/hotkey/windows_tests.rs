//! Opt-in hook test: target a benign fixture, never the user's terminal/CLI.
use super::*;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::HWND,
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{AttachThreadInput, GetCurrentThreadId},
    },
    UI::{
        Input::KeyboardAndMouse::{SetFocus, VK_SPACE},
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetForegroundWindow,
            GetWindowThreadProcessId, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SW_SHOW,
            SetForegroundWindow, ShowWindow, TranslateMessage, UnregisterClassW, WM_CHAR,
            WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW, WS_OVERLAPPEDWINDOW,
        },
    },
};
const CLASS: *const u16 = windows_sys::core::w!("RustickersShortcutSuppressionTest");
static LEAKS: AtomicUsize = AtomicUsize::new(0);
unsafe extern "system" fn source_proc(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if (matches!(message, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP)
        && wparam == usize::from(VK_SPACE))
        || (message == WM_CHAR && wparam == 32)
    {
        LEAKS.fetch_add(1, Ordering::SeqCst);
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
struct Fixture {
    hwnd: HWND,
    foreground: HWND,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for key in [
            rdev::Key::Space,
            rdev::Key::ShiftLeft,
            rdev::Key::ControlLeft,
        ] {
            let _ = rdev::simulate(&rdev::EventType::KeyRelease(key));
        }
        unsafe {
            SetForegroundWindow(self.foreground);
            DestroyWindow(self.hwnd);
            UnregisterClassW(CLASS, GetModuleHandleW(std::ptr::null()));
        }
    }
}
fn pump(listener: &mut windows_listener::Listener) {
    let mut message = MSG::default();
    while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
        unsafe {
            TranslateMessage(&message);
        }
        listener.process(&message);
    }
}
fn settle(listener: &mut windows_listener::Listener) {
    let until = Instant::now() + Duration::from_millis(40);
    while Instant::now() < until {
        pump(listener);
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
#[ignore = "opt-in Windows hook test: focuses an isolated window and verifies shortcut keys never reach it"]
fn selection_shortcut_suppresses_space_and_trailing_release_natively() -> anyhow::Result<()> {
    crate::native::clipboard::paste::wait_for_modifiers()?;
    let config = tempfile::tempdir()?;
    bindings::configure(&config.path().join("hotkeys.json"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    WINDOWS_CONTEXT
        .set(std::sync::Mutex::new(WindowsContext {
            tx,
            matcher: Matcher::default(),
        }))
        .map_err(|_| anyhow::anyhow!("run this native hook test in its own process"))?;
    let foreground = unsafe { GetForegroundWindow() };
    anyhow::ensure!(
        !foreground.is_null(),
        "Unlock Windows and activate a window before running this desktop test"
    );
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = WNDCLASSW {
        lpfnWndProc: Some(source_proc),
        hInstance: instance,
        lpszClassName: CLASS,
        ..Default::default()
    };
    anyhow::ensure!(unsafe { RegisterClassW(&class) } != 0);
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            CLASS,
            CLASS,
            WS_OVERLAPPEDWINDOW,
            40,
            40,
            320,
            200,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    anyhow::ensure!(!hwnd.is_null());
    let _fixture = Fixture { hwnd, foreground };
    let mut listener = windows_listener::Listener::new()?;
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        let current = GetCurrentThreadId();
        let other = GetWindowThreadProcessId(foreground, std::ptr::null_mut());
        AttachThreadInput(current, other, 1);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
        AttachThreadInput(current, other, 0);
    }
    settle(&mut listener);
    anyhow::ensure!(
        unsafe { GetForegroundWindow() } == hwnd,
        "fixture did not receive focus"
    );
    for trailing_space in [false, true] {
        LEAKS.store(0, Ordering::SeqCst);
        for key in [
            rdev::Key::ControlLeft,
            rdev::Key::ShiftLeft,
            rdev::Key::Space,
            rdev::Key::Space,
        ] {
            rdev::simulate(&rdev::EventType::KeyPress(key))
                .map_err(|err| anyhow::anyhow!("{err:?}"))?;
            settle(&mut listener);
        }
        let releases = if trailing_space {
            [
                rdev::Key::ShiftLeft,
                rdev::Key::ControlLeft,
                rdev::Key::Space,
            ]
        } else {
            [
                rdev::Key::Space,
                rdev::Key::ShiftLeft,
                rdev::Key::ControlLeft,
            ]
        };
        for key in releases {
            rdev::simulate(&rdev::EventType::KeyRelease(key))
                .map_err(|err| anyhow::anyhow!("{err:?}"))?;
            settle(&mut listener);
        }
        let actions: Vec<_> = rx.try_iter().collect();
        anyhow::ensure!(
            matches!(actions.as_slice(), [IpcEvent::TriggerSelectionToCommand]),
            "expected one selection action, received {} event(s)",
            actions.len()
        );
        anyhow::ensure!(
            LEAKS.load(Ordering::SeqCst) == 0,
            "shortcut Space leaked to source"
        );
        eprintln!(
            "Ctrl+Shift+Space dispatched once, with no source key/character leak (trailing_space={trailing_space})"
        );
    }
    // Ordinary Space must still work after the consumed chord has ended.
    rdev::simulate(&rdev::EventType::KeyPress(rdev::Key::Space)).unwrap();
    settle(&mut listener);
    rdev::simulate(&rdev::EventType::KeyRelease(rdev::Key::Space)).unwrap();
    settle(&mut listener);
    anyhow::ensure!(
        LEAKS.load(Ordering::SeqCst) > 0,
        "ordinary Space was incorrectly suppressed"
    );
    Ok(())
}
