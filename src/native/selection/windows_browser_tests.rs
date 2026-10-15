//! Real Chromium accessibility smoke test, with a disposable browser profile.
use super::*;
use std::{
    os::windows::process::CommandExt,
    process::{Child, Command, Stdio},
};
use windows_sys::Win32::{
    Foundation::HWND,
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{AttachThreadInput, GetCurrentThreadId},
    },
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows,
        GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, MSG,
        PM_REMOVE, PeekMessageW, RegisterClassW, SW_RESTORE, SW_SHOW, SetForegroundWindow,
        ShowWindow, TranslateMessage, UnregisterClassW, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    },
};
const CLASS: *const u16 = windows_sys::core::w!("RustickersBrowserSelectionTest");
const TEXT: &str = "isolated browser selection regression";
const EDITOR_TEXT: &str = "isolated multiline browser selection";
const INPUT_TEXT: &str = "isolated input browser selection";
struct Foreground {
    window: HWND,
    previous: HWND,
}
impl Drop for Foreground {
    fn drop(&mut self) {
        unsafe {
            SetForegroundWindow(self.previous);
            DestroyWindow(self.window);
            UnregisterClassW(CLASS, GetModuleHandleW(std::ptr::null()));
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
struct BrowserWindow {
    pid: u32,
    hwnd: HWND,
}
unsafe extern "system" fn find_browser_window(hwnd: HWND, state: isize) -> i32 {
    let state = unsafe { &mut *(state as *mut BrowserWindow) };
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
    }
    if pid == state.pid && unsafe { IsWindowVisible(hwnd) } != 0 {
        let mut title = [0u16; 256];
        let len = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
        if String::from_utf16_lossy(&title[..len.max(0) as usize])
            .contains("Isolated selection capture test")
        {
            state.hwnd = hwnd;
            return 0;
        }
    }
    1
}
struct Browser(Child);
impl Drop for Browser {
    fn drop(&mut self) {
        // The unique profile guarantees this process is not the user's browser.
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = Command::new("taskkill.exe")
                .args(["/PID", &self.0.id().to_string(), "/T", "/F"])
                .creation_flags(0x08000000) // CREATE_NO_WINDOW: cleanup must not steal focus.
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = self.0.wait();
        }
    }
}
#[test]
#[ignore = "opt-in Windows browser test: launches isolated Chrome/Edge profiles; restores foreground; never writes clipboard"]
fn reads_browser_selection_and_empty_state_without_copy() -> anyhow::Result<()> {
    anyhow::ensure!(
        !unsafe { GetForegroundWindow() }.is_null(),
        "Unlock Windows and activate a window before running this desktop test"
    );
    let paths = [
        std::path::PathBuf::from("C:/Program Files/Google/Chrome/Application/chrome.exe"),
        std::path::PathBuf::from("C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"),
    ];
    let available: Vec<_> = paths.into_iter().filter(|path| path.exists()).collect();
    anyhow::ensure!(
        !available.is_empty(),
        "Install Chrome or Edge to run this opt-in test"
    );
    initialize();
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = WNDCLASSW {
        lpfnWndProc: Some(DefWindowProcW),
        hInstance: instance,
        lpszClassName: CLASS,
        ..Default::default()
    };
    anyhow::ensure!(unsafe { RegisterClassW(&class) } != 0);
    let window = unsafe {
        CreateWindowExW(
            0,
            CLASS,
            CLASS,
            WS_OVERLAPPEDWINDOW,
            40,
            40,
            300,
            200,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    anyhow::ensure!(!window.is_null());
    let previous = unsafe { GetForegroundWindow() };
    let _foreground = Foreground { window, previous };
    for path in available {
        unsafe {
            ShowWindow(window, SW_SHOW);
            let current = GetCurrentThreadId();
            let other = GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut());
            AttachThreadInput(current, other, 1);
            SetForegroundWindow(window);
            AttachThreadInput(current, other, 0);
        }
        let profile = tempfile::Builder::new()
            .prefix("rustickers-browser-selection-")
            .tempdir()?;
        let page = profile.path().join("selection.html");
        std::fs::write(
            &page,
            format!(
                r#"<!doctype html><html><head><title>Isolated selection capture test</title></head>
<body><p id="sample" tabindex="0">{TEXT}</p>
<textarea id="editor">{EDITOR_TEXT}</textarea><input id="single" value="{INPUT_TEXT}"><script>
onload = () => {{ const sample = document.getElementById('sample'); sample.focus();
setTimeout(() => window.getSelection().selectAllChildren(sample), 8000);
setTimeout(() => {{ window.getSelection().removeAllRanges(); const editor = document.getElementById('editor'); editor.focus(); editor.setSelectionRange(0, 0); }}, 14000);
setTimeout(() => document.getElementById('editor').select(), 16000);
setTimeout(() => {{ const single = document.getElementById('single'); single.focus(); single.setSelectionRange(0, 0); }}, 22000);
setTimeout(() => document.getElementById('single').select(), 24000); }};
</script></body></html>"#
            ),
        )?;
        let page_url =
            url::Url::from_file_path(&page).map_err(|_| anyhow::anyhow!("invalid fixture URL"))?;
        let child = Command::new(&path)
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                "--new-window",
            ])
            .arg(page_url.as_str())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let mut browser = Browser(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        let source = loop {
            pump();
            let hwnd = unsafe { GetForegroundWindow() };
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(hwnd, &mut pid);
            }
            if pid == browser.0.id() {
                break hwnd as u64;
            }
            anyhow::ensure!(
                browser.0.try_wait()?.is_none(),
                "isolated browser exited before opening its window"
            );
            let mut target = BrowserWindow {
                pid: browser.0.id(),
                hwnd: std::ptr::null_mut(),
            };
            unsafe {
                EnumWindows(
                    Some(find_browser_window),
                    &mut target as *mut BrowserWindow as isize,
                );
            }
            if !target.hwnd.is_null() {
                unsafe {
                    let current = GetCurrentThreadId();
                    let other = GetWindowThreadProcessId(target.hwnd, std::ptr::null_mut());
                    AttachThreadInput(current, other, 1);
                    ShowWindow(target.hwnd, SW_RESTORE);
                    SetForegroundWindow(target.hwnd);
                    AttachThreadInput(current, other, 0);
                }
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "isolated browser did not become foreground (browser={}, foreground={pid}, found_window={:?})",
                browser.0.id(),
                target.hwnd
            );
            thread::sleep(Duration::from_millis(20));
        };
        // Wait for the real provider to initialize, not a forced accessibility
        // flag. A cold browser can expose its top-level frame before its page.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            pump();
            let read = windows_accessibility::read(source);
            if read == windows_accessibility::SelectionRead::Empty {
                break;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "browser did not expose empty TextPattern: {read:?}"
            );
            thread::sleep(Duration::from_millis(30));
        }
        let generation = crate::native::clipboard::watcher::clipboard_generation();
        let started = Instant::now();
        anyhow::ensure!(
            capture_selection_from_window(&CaptureFilter::new(), Some(source))?.is_none()
        );
        anyhow::ensure!(
            started.elapsed() < Duration::from_millis(250),
            "empty browser capture was slow: {:?}",
            started.elapsed()
        );
        eprintln!(
            "{} empty selection returned in {:?}",
            path.file_name().unwrap().to_string_lossy(),
            started.elapsed()
        );
        for expected in [Some(TEXT), None, Some(EDITOR_TEXT), None, Some(INPUT_TEXT)] {
            let expected_read = expected
                .map(|text| windows_accessibility::SelectionRead::Text(text.into()))
                .unwrap_or(windows_accessibility::SelectionRead::Empty);
            let deadline = Instant::now() + Duration::from_secs(12);
            loop {
                pump();
                let read = windows_accessibility::read(source);
                if read == expected_read {
                    break;
                }
                anyhow::ensure!(
                    Instant::now() < deadline,
                    "browser selected text not readable: {read:?}, expected {expected:?}"
                );
                thread::sleep(Duration::from_millis(50));
            }
            let started = Instant::now();
            anyhow::ensure!(
                capture_selection_from_window(&CaptureFilter::new(), Some(source))?.as_deref()
                    == expected
            );
            anyhow::ensure!(
                started.elapsed() < Duration::from_millis(250),
                "browser document/input/textarea capture was slow: {:?}",
                started.elapsed()
            );
        }
        anyhow::ensure!(
            crate::native::clipboard::watcher::clipboard_generation() == generation,
            "accessibility capture touched clipboard"
        );
        anyhow::ensure!(
            unsafe { GetForegroundWindow() } as u64 == source,
            "capture stole focus"
        );
        drop(browser);
    }
    Ok(())
}
