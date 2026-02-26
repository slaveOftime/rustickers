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
                CoUninitialize, IServiceProvider,
            },
            Variant::VARIANT,
        },
        UI::{
            Shell::{
                IShellBrowser, IShellFolderViewDual, IShellWindows, IWebBrowserApp,
                SID_STopLevelBrowser, ShellWindows,
            },
            WindowsAndMessaging::{FindWindowExW, GetForegroundWindow},
        },
    },
    core::Interface,
    core::w,
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
fn find_descendant_window_by_class(parent: HWND, class_name: windows::core::PCWSTR) -> HWND {
    let direct = unsafe { FindWindowExW(Some(parent), None, class_name, None) }
        .unwrap_or(HWND(std::ptr::null_mut()));
    if !direct.0.is_null() {
        return direct;
    }

    let mut child = unsafe { FindWindowExW(Some(parent), None, None, None) }
        .unwrap_or(HWND(std::ptr::null_mut()));
    while !child.0.is_null() {
        let nested = find_descendant_window_by_class(child, class_name);
        if !nested.0.is_null() {
            return nested;
        }

        child = unsafe { FindWindowExW(Some(parent), Some(child), None, None) }
            .unwrap_or(HWND(std::ptr::null_mut()));
    }

    HWND(std::ptr::null_mut())
}

#[cfg(target_os = "windows")]
fn get_active_explorer_tab(foreground_window: HWND) -> HWND {
    let shell_tab = find_descendant_window_by_class(foreground_window, w!("ShellTabWindowClass"));
    if !shell_tab.0.is_null() {
        return shell_tab;
    }

    find_descendant_window_by_class(foreground_window, w!("TabWindowClass"))
}

#[cfg(target_os = "windows")]
fn get_shell_browser_window(browser: &IWebBrowserApp) -> Option<HWND> {
    let service_provider: IServiceProvider = browser.cast().ok()?;
    let shell_browser: IShellBrowser =
        unsafe { service_provider.QueryService(&SID_STopLevelBrowser) }.ok()?;
    unsafe { shell_browser.GetWindow() }.ok()
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

    let active_tab = get_active_explorer_tab(foreground_window);

    let shell_windows: IShellWindows =
        unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_ALL) }?;
    let count = unsafe { shell_windows.Count()? };

    let mut fallback_paths: Vec<PathBuf> = Vec::new();
    let mut foreground_fallback_paths: Vec<PathBuf> = Vec::new();

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

        if browser_hwnd == foreground_window {
            if active_tab.0.is_null() {
                return Ok(current_paths);
            }

            if let Some(shell_browser_window) = get_shell_browser_window(&browser) {
                if shell_browser_window == active_tab {
                    return Ok(current_paths);
                }
            }

            if foreground_fallback_paths.is_empty() {
                foreground_fallback_paths = current_paths;
            }

            continue;
        }

        if fallback_paths.is_empty() {
            fallback_paths = current_paths;
        }
    }

    if !foreground_fallback_paths.is_empty() {
        return Ok(foreground_fallback_paths);
    }

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
