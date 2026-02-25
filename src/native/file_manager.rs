use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(target_os = "windows")]
use windows::{
    Win32::{
        Foundation::{HWND, SHANDLE_PTR},
        System::{
            Com::{
                CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            Variant::VARIANT,
        },
        UI::{
            Shell::{IShellFolderViewDual, IShellWindows, IWebBrowserApp, ShellWindows},
            WindowsAndMessaging::GetForegroundWindow,
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

    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null() {
        return Ok(Vec::new());
    }

    let shell_windows: IShellWindows =
        unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_ALL) }?;
    let count = unsafe { shell_windows.Count()? };

    let mut matched_paths: Vec<PathBuf> = Vec::new();
    let mut fallback_paths: Vec<PathBuf> = Vec::new();

    for i in 0..count {
        let index = VARIANT::from(i);
        let dispatch = unsafe { shell_windows.Item(&index)? };
        let browser: IWebBrowserApp = match dispatch.cast() {
            Ok(browser) => browser,
            Err(_) => continue,
        };

        let hwnd = HWND(unsafe { browser.HWND()? }.0 as *mut _);

        let document = match unsafe { browser.Document() } {
            Ok(document) => document,
            Err(_) => continue,
        };

        let folder_view: IShellFolderViewDual = match document.cast() {
            Ok(view) => view,
            Err(_) => continue,
        };

        let selected_items = match unsafe { folder_view.SelectedItems() } {
            Ok(items) => items,
            Err(_) => continue,
        };

        let selected_count = match unsafe { selected_items.Count() } {
            Ok(count) => count,
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

            let path = match unsafe { folder_item.Path() } {
                Ok(path) => path,
                Err(_) => continue,
            };

            let path = path.to_string();
            if !path.trim().is_empty() {
                current_paths.push(PathBuf::from(path));
            }
        }

        if current_paths.is_empty() {
            continue;
        }

        if same_window(hwnd, foreground) {
            matched_paths = current_paths;
            break;
        }

        if fallback_paths.is_empty() {
            fallback_paths = current_paths;
        }
    }

    if !matched_paths.is_empty() {
        return Ok(matched_paths);
    }

    Ok(fallback_paths)
}

#[cfg(target_os = "windows")]
fn same_window(lhs: HWND, rhs: HWND) -> bool {
    let lhs = SHANDLE_PTR(lhs.0 as isize);
    let rhs = SHANDLE_PTR(rhs.0 as isize);
    lhs == rhs
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
