//! Opt-in native selection capture regression; no application data is used.
use super::*;
use std::{
    sync::atomic::{AtomicU32, Ordering},
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::HWND,
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, GetKeyState, SetFocus, VK_CONTROL, VK_SHIFT},
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
            GetShellWindow, GetWindowThreadProcessId, KillTimer, MSG, PM_REMOVE, PeekMessageW,
            RegisterClassW, SW_SHOW, SendMessageW, SetForegroundWindow, SetTimer, ShowWindow,
            TranslateMessage, UnregisterClassW, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
            WM_TIMER, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        },
    },
};

const SELECTED: &str = "isolated selection capture regression — selected text";
const CLASS: *const u16 = windows_sys::core::w!("RustickersSelectionCaptureTest");
static COPY_DELAY: AtomicU32 = AtomicU32::new(0);
static COPY_COUNT: AtomicU32 = AtomicU32::new(0);

fn write_selection() {
    arboard::Clipboard::new()
        .unwrap()
        .set_text(SELECTED)
        .unwrap();
}
unsafe extern "system" fn source_proc(
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            // Behave like an app that binds Ctrl+C, but not Ctrl+Shift+C.
            if wparam == 0x43
                && unsafe { GetKeyState(VK_CONTROL as i32) } < 0
                && unsafe { GetKeyState(VK_SHIFT as i32) } >= 0
            {
                COPY_COUNT.fetch_add(1, Ordering::SeqCst);
                let delay = COPY_DELAY.load(Ordering::SeqCst);
                if delay == 0 {
                    write_selection();
                } else if delay != u32::MAX {
                    unsafe { SetTimer(hwnd, 1, delay, None) };
                }
            }
            return 0;
        }
        WM_KEYUP | WM_SYSKEYUP => return 0,
        WM_TIMER if wparam == 1 => {
            unsafe { KillTimer(hwnd, 1) };
            write_selection();
            return 0;
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

struct Fixture {
    class: *const u16,
    hwnd: HWND,
    foreground: HWND,
    clipboard: Option<String>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = simulate(&EventType::KeyRelease(Key::KeyC));
        let _ = simulate(&EventType::KeyRelease(Key::ShiftLeft));
        let _ = simulate(&EventType::KeyRelease(Key::ControlLeft));
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            match self.clipboard.take() {
                Some(text) => {
                    let _ = clipboard.set_text(text);
                }
                None => {
                    let _ = clipboard.clear();
                }
            }
        }
        unsafe {
            SetForegroundWindow(self.foreground);
            DestroyWindow(self.hwnd);
            UnregisterClassW(self.class, GetModuleHandleW(std::ptr::null()));
        }
    }
}
fn pump() {
    let mut message = MSG::default();
    while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        pump();
        if ready() {
            return;
        }
        assert!(Instant::now() < deadline, "selection test timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "opt-in Windows desktop test: focuses an isolated source window and temporarily writes clipboard text"]
fn captures_after_shortcut_release_and_slow_copy() -> anyhow::Result<()> {
    anyhow::ensure!(
        crate::native::clipboard::paste::wait_for_modifiers().is_ok(),
        "release modifiers before testing"
    );
    let foreground = unsafe { GetForegroundWindow() };
    anyhow::ensure!(
        !foreground.is_null(),
        "Unlock Windows and activate a window before running this desktop test"
    );
    let previous = arboard::Clipboard::new()?.get_text().ok();
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = WNDCLASSW {
        lpfnWndProc: Some(source_proc),
        hInstance: instance,
        lpszClassName: CLASS,
        ..Default::default()
    };
    anyhow::ensure!(
        unsafe { RegisterClassW(&class) } != 0,
        "register source window"
    );
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
    anyhow::ensure!(!hwnd.is_null(), "create source window");
    let _fixture = Fixture {
        class: CLASS,
        hwnd,
        foreground,
        clipboard: previous,
    };
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        // Test setup only: share input queues briefly so the isolated fixture
        // can be activated even when the test runner is not foreground.
        let current = GetCurrentThreadId();
        let foreground_thread = GetWindowThreadProcessId(foreground, std::ptr::null_mut());
        windows_sys::Win32::System::Threading::AttachThreadInput(current, foreground_thread, 1);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
        windows_sys::Win32::System::Threading::AttachThreadInput(current, foreground_thread, 0);
    }
    wait_until(|| unsafe { GetForegroundWindow() } == hwnd);

    for (held_modifiers, delay) in [(true, 0), (false, 350), (false, u32::MAX)] {
        arboard::Clipboard::new()?.set_text("clipboard sentinel")?;
        COPY_DELAY.store(delay, Ordering::SeqCst);
        COPY_COUNT.store(0, Ordering::SeqCst);
        if held_modifiers {
            simulate(&EventType::KeyPress(Key::ControlLeft)).unwrap();
            simulate(&EventType::KeyPress(Key::ShiftLeft)).unwrap();
            wait_until(|| unsafe { GetAsyncKeyState(VK_CONTROL as i32) } < 0
                && unsafe { GetAsyncKeyState(VK_SHIFT as i32) } < 0);
        }
        let worker = thread::spawn(capture_selection);
        let release_at = Instant::now() + Duration::from_millis(150);
        let mut released = !held_modifiers;
        wait_until(|| {
            if !released && Instant::now() >= release_at {
                simulate(&EventType::KeyRelease(Key::ShiftLeft)).unwrap();
                simulate(&EventType::KeyRelease(Key::ControlLeft)).unwrap();
                released = true;
            }
            worker.is_finished() && released
        });
        let captured = worker.join().unwrap()?;
        let expected = (delay != u32::MAX).then_some(SELECTED);
        anyhow::ensure!(
            captured.as_deref() == expected,
            "capture failed with held_modifiers={held_modifiers}, copy_delay={delay}ms: {captured:?}"
        );
        anyhow::ensure!(
            COPY_COUNT.load(Ordering::SeqCst) == 1,
            "expected one plain Ctrl+C"
        );
        anyhow::ensure!(
            arboard::Clipboard::new()?.get_text()? == "clipboard sentinel",
            "restore previous clipboard"
        );
        anyhow::ensure!(
            unsafe { GetForegroundWindow() } == hwnd,
            "capture must not steal focus"
        );
        eprintln!(
            "Verified selection capture: held_modifiers={held_modifiers}, copy_delay={delay}ms, has_selection={}",
            captured.is_some()
        );
    }

    // A native edit with only a caret has a definitive empty selection. Do
    // not inject Copy or wait out the clipboard timeout in this common case.
    let value: Vec<u16> = SELECTED.encode_utf16().chain(Some(0)).collect();
    let edit = unsafe {
        CreateWindowExW(
            0,
            windows_sys::core::w!("Edit"),
            value.as_ptr(),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            300,
            100,
            hwnd,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    anyhow::ensure!(!edit.is_null(), "create native edit fixture");
    unsafe {
        SetFocus(edit);
        SendMessageW(edit, 0xb1, 0, 0);
    } // EM_SETSEL: caret only.
    pump();
    let generation = crate::native::clipboard::watcher::clipboard_generation();
    let started = Instant::now();
    let worker = thread::spawn(capture_selection);
    wait_until(|| worker.is_finished());
    anyhow::ensure!(worker.join().unwrap()?.is_none());
    let elapsed = started.elapsed();
    anyhow::ensure!(
        elapsed < Duration::from_millis(150),
        "empty native selection took {elapsed:?}"
    );
    anyhow::ensure!(
        crate::native::clipboard::watcher::clipboard_generation() == generation,
        "empty selection must leave the clipboard untouched"
    );
    eprintln!("Empty native selection returned in {elapsed:?}");

    // The same edit must still capture selected text, using accessibility when
    // available or retaining the clipboard fallback for legacy native controls.
    unsafe {
        SendMessageW(edit, 0xb1, 0, -1);
    } // EM_SETSEL: select all.
    let worker = thread::spawn(capture_selection);
    wait_until(|| worker.is_finished());
    anyhow::ensure!(worker.join().unwrap()?.as_deref() == Some(SELECTED));
    unsafe {
        SetFocus(hwnd);
        DestroyWindow(edit);
    }

    // A terminal without an accessibility selection must never receive Ctrl+C
    // (which would interrupt a real CLI). Use terminal-class fixture windows,
    // not the user's terminal, with Copy counted as an observable signal.
    for (ix, terminal_class) in [
        windows_sys::core::w!("ConsoleWindowClass"),
        windows_sys::core::w!("CASCADIA_HOSTING_WINDOW_CLASS"),
    ]
    .into_iter()
    .enumerate()
    {
        let class = WNDCLASSW {
            lpfnWndProc: Some(source_proc),
            hInstance: instance,
            lpszClassName: terminal_class,
            ..Default::default()
        };
        anyhow::ensure!(unsafe { RegisterClassW(&class) } != 0);
        let terminal = unsafe {
            CreateWindowExW(
                0,
                terminal_class,
                terminal_class,
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
        anyhow::ensure!(!terminal.is_null());
        let restore = Fixture {
            class: terminal_class,
            hwnd: terminal,
            foreground: hwnd,
            clipboard: arboard::Clipboard::new()?.get_text().ok(),
        };
        unsafe {
            ShowWindow(terminal, SW_SHOW);
            SetForegroundWindow(terminal);
            SetFocus(terminal);
        }
        wait_until(|| unsafe { GetForegroundWindow() } == terminal);
        if ix == 0 {
            simulate(&EventType::KeyPress(Key::ControlLeft)).unwrap();
            simulate(&EventType::KeyPress(Key::ShiftLeft)).unwrap();
            wait_until(|| unsafe { GetAsyncKeyState(VK_CONTROL as i32) } < 0
                && unsafe { GetAsyncKeyState(VK_SHIFT as i32) } < 0);
        }
        COPY_COUNT.store(0, Ordering::SeqCst);
        let generation = crate::native::clipboard::watcher::clipboard_generation();
        let started = Instant::now();
        let worker = thread::spawn(capture_selection);
        wait_until(|| worker.is_finished());
        anyhow::ensure!(worker.join().unwrap()?.is_none());
        anyhow::ensure!(
            started.elapsed() < Duration::from_millis(250),
            "terminal capture was slow: {:?}",
            started.elapsed()
        );
        anyhow::ensure!(
            COPY_COUNT.load(Ordering::SeqCst) == 0,
            "terminal received Ctrl+C"
        );
        anyhow::ensure!(crate::native::clipboard::watcher::clipboard_generation() == generation);
        anyhow::ensure!(unsafe { GetForegroundWindow() } == terminal);
        eprintln!("Terminal returned without Copy in {:?}", started.elapsed());
        if ix == 0 {
            anyhow::ensure!(
                unsafe { GetAsyncKeyState(VK_CONTROL as i32) } < 0
                    && unsafe { GetAsyncKeyState(VK_SHIFT as i32) } < 0,
                "capture must not synthesize modifier releases"
            );
            simulate(&EventType::KeyRelease(Key::ShiftLeft)).unwrap();
            simulate(&EventType::KeyRelease(Key::ControlLeft)).unwrap();
        }
        drop(restore);
        wait_until(|| unsafe { GetForegroundWindow() } == hwnd);
    }

    // No foreground application is already a definitive no-selection result.
    let generation = crate::native::clipboard::watcher::clipboard_generation();
    let started = Instant::now();
    anyhow::ensure!(capture_selection_from_window(&CaptureFilter::new(), None)?.is_none());
    anyhow::ensure!(started.elapsed() < Duration::from_millis(150));
    anyhow::ensure!(crate::native::clipboard::watcher::clipboard_generation() == generation);

    // Activate the real desktop without clicking icons or sending Copy. This
    // validates shell classification, not just a made-up window class string.
    let desktop = unsafe { GetShellWindow() };
    if !desktop.is_null() {
        unsafe {
            let current = GetCurrentThreadId();
            let desktop_thread = GetWindowThreadProcessId(desktop, std::ptr::null_mut());
            windows_sys::Win32::System::Threading::AttachThreadInput(current, desktop_thread, 1);
            SetForegroundWindow(desktop);
            windows_sys::Win32::System::Threading::AttachThreadInput(current, desktop_thread, 0);
        }
        wait_until(|| unsafe { GetForegroundWindow() } == desktop);
        let generation = crate::native::clipboard::watcher::clipboard_generation();
        let started = Instant::now();
        let worker = thread::spawn(capture_selection);
        wait_until(|| worker.is_finished());
        anyhow::ensure!(worker.join().unwrap()?.is_none());
        let elapsed = started.elapsed();
        anyhow::ensure!(
            elapsed < Duration::from_millis(150),
            "desktop capture took {elapsed:?}"
        );
        anyhow::ensure!(crate::native::clipboard::watcher::clipboard_generation() == generation);
        eprintln!("Desktop selection returned in {elapsed:?}");
        unsafe {
            SetForegroundWindow(hwnd);
            SetFocus(hwnd);
        }
        wait_until(|| unsafe { GetForegroundWindow() } == hwnd);
    } else {
        eprintln!("Desktop latency check skipped: this session has no shell window");
    }

    // A queued request whose source no longer has focus must not send Copy or
    // write its marker into the current foreground application's clipboard.
    let copies = COPY_COUNT.load(Ordering::SeqCst);
    anyhow::ensure!(capture_selection_from_window(&CaptureFilter::new(), Some(0)).is_err());
    anyhow::ensure!(COPY_COUNT.load(Ordering::SeqCst) == copies);
    anyhow::ensure!(arboard::Clipboard::new()?.get_text()? == "clipboard sentinel");
    Ok(())
}
