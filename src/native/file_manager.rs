use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(target_os = "windows")]
use windows::{
    Win32::{
        Foundation::HWND,
        System::{
            Com::{
                CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            Variant::VARIANT,
        },
        UI::{
            Shell::{IShellFolderViewDual, IShellWindows, IWebBrowserApp, ShellWindows},
            WindowsAndMessaging::{
                GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId,
                IsChild,
            },
        },
    },
    core::Interface,
};

pub fn selected_files_from_active_manager() -> anyhow::Result<Vec<PathBuf>> {
    #[cfg(target_os = "windows")]
    {
        return selected_files_windows();
    }

    #[cfg(target_os = "macos")]
    {
        return selected_files_macos();
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Ok(Vec::new())
    }
}

#[cfg(target_os = "windows")]
fn selected_files_windows() -> anyhow::Result<Vec<PathBuf>> {
    struct ComGuard;
    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() }
        }
    }

    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    let _com_guard = ComGuard;

    let foreground_window = unsafe { GetForegroundWindow() };
    if foreground_window.0.is_null() {
        return Ok(Vec::new());
    }

    // 1. Get the actual focused element (e.g., the specific list view inside the active tab)
    //    We need this because 'foreground_window' is just the outer Frame container.
    let mut focused_hwnd = HWND(std::ptr::null_mut());
    let id = unsafe { GetWindowThreadProcessId(foreground_window, None) };

    let mut gui_info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };

    if unsafe { GetGUIThreadInfo(id, &mut gui_info).is_ok() } {
        focused_hwnd = gui_info.hwndFocus;
    }

    let shell_windows: IShellWindows =
        unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_ALL) }?;
    let count = unsafe { shell_windows.Count()? };

    let mut matched_paths: Vec<PathBuf> = Vec::new();
    let mut fallback_paths: Vec<PathBuf> = Vec::new();

    for i in 0..count {
        let index = VARIANT::from(i);
        let dispatch = match unsafe { shell_windows.Item(&index) } {
            Ok(d) => d,
            Err(_) => continue,
        };

        let browser: IWebBrowserApp = match dispatch.cast() {
            Ok(b) => b,
            Err(_) => continue,
        };

        // The HWND of the specific Tab/Browser View
        let browser_hwnd = match unsafe { browser.HWND() } {
            Ok(h) => HWND(h.0 as *mut _),
            Err(_) => continue,
        };

        let document = match unsafe { browser.Document() } {
            Ok(d) => d,
            Err(_) => continue,
        };

        let folder_view: IShellFolderViewDual = match document.cast() {
            Ok(v) => v,
            Err(_) => continue,
        };

        let selected_items = match unsafe { folder_view.SelectedItems() } {
            Ok(i) => i,
            Err(_) => continue,
        };

        let selected_count = match unsafe { selected_items.Count() } {
            Ok(c) => c,
            Err(_) => continue,
        };

        if selected_count <= 0 {
            continue;
        }

        let mut current_paths: Vec<PathBuf> = Vec::new();
        for idx in 0..selected_count {
            let item_index = VARIANT::from(idx);
            let item_dispatch = match unsafe { selected_items.Item(&item_index) } {
                Ok(item) => item,
                Err(_) => continue,
            };

            let folder_item: windows::Win32::UI::Shell::FolderItem = match item_dispatch.cast() {
                Ok(item) => item,
                Err(_) => continue,
            };

            let path_bs = match unsafe { folder_item.Path() } {
                Ok(path) => path,
                Err(_) => continue,
            };

            let path_str = path_bs.to_string();
            if !path_str.trim().is_empty() {
                current_paths.push(PathBuf::from(path_str));
            }
        }

        if current_paths.is_empty() {
            continue;
        }

        // --- FIX LOGIC START ---
        // We check if this specific browser instance is the one the user is interacting with.
        let is_active_tab = if browser_hwnd == foreground_window {
            // Case: Legacy Explorer or Single Window mode where Frame == View
            true
        } else if !focused_hwnd.0.is_null() {
            // Case: Tabbed Explorer.
            // The 'focused_hwnd' (e.g., file list) should be a child of the 'browser_hwnd' (the tab).
            browser_hwnd == focused_hwnd || unsafe { IsChild(browser_hwnd, focused_hwnd) }.as_bool()
        } else {
            false
        };

        if is_active_tab {
            matched_paths = current_paths;
            break;
        }
        // --- FIX LOGIC END ---

        if fallback_paths.is_empty() {
            fallback_paths = current_paths;
        }
    }

    if !matched_paths.is_empty() {
        return Ok(matched_paths);
    }

    // Fallback: If we couldn't determine the active tab (e.g. focus was on title bar),
    // return the first one we found that had files selected.
    Ok(fallback_paths)
}

#[cfg(target_os = "macos")]
fn selected_files_macos() -> anyhow::Result<Vec<PathBuf>> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg("tell application \"System Events\" to set frontApp to name of first application process whose frontmost is true")
        .arg("-e")
        .arg("if frontApp is not \"Finder\" then return \"\"")
        .arg("-e")
        .arg("tell application \"Finder\"")
        .arg("-e")
        .arg("set selectedItems to selection")
        .arg("-e")
        .arg("if (count of selectedItems) is 0 then return \"\"")
        .arg("-e")
        .arg("set outputText to \"\"")
        .arg("-e")
        .arg("repeat with anItem in selectedItems")
        .arg("-e")
        .arg("set outputText to outputText & POSIX path of (anItem as alias) & linefeed")
        .arg("-e")
        .arg("end repeat")
        .arg("-e")
        .arg("return outputText")
        .arg("-e")
        .arg("end tell")
        .output()?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect::<Vec<_>>();

    Ok(files)
}
