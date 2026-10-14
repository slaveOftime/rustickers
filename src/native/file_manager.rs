use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::{ffi::CStr, os::raw::c_char, ptr};

#[cfg(target_os = "macos")]
use cocoa::foundation::{NSAutoreleasePool, NSString};

#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};

#[cfg(target_os = "macos")]
use objc::runtime::Object;

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
                SID_STopLevelBrowser, SVGIO_BACKGROUND, SWC_DESKTOP, SWFO_NEEDDISPATCH,
                ShellWindows,
            },
            WindowsAndMessaging::{FindWindowExW, GetClassNameW, GetForegroundWindow},
        },
    },
    core::Interface,
    core::w,
};

/// Resolve the foreground file manager or Windows desktop selection without a GPUI borrow.
/// Windows shell automation can dispatch UI callbacks while waiting for COM/RPC.
pub fn selected_files_from_active_manager() -> anyhow::Result<Vec<PathBuf>> {
    #[cfg(target_os = "windows")]
    {
        selected_files_windows()
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
    selected_files_windows_for_window(unsafe { GetForegroundWindow() })
}

#[cfg(target_os = "windows")]
fn selected_files_windows_for_window(foreground_window: HWND) -> anyhow::Result<Vec<PathBuf>> {
    struct ComGuard;
    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() }
        }
    }

    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    let _com_guard = ComGuard;

    if foreground_window.0.is_null() {
        return Ok(Vec::new());
    }

    let active_tab = get_active_explorer_tab(foreground_window);

    let shell_windows: IShellWindows =
        unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_ALL) }?;
    // The desktop is not an ordinary ShellWindows entry. Its browser service
    // exposes the same folder-view automation through the active Shell view.
    if is_desktop_window(foreground_window) {
        let folder_view = desktop_folder_view(&shell_windows)?;
        let mut paths = selected_paths(&folder_view)?;
        // Virtual icons (This PC, Recycle Bin, etc.) have namespace paths, not
        // previewable files. Preserve selected file/shortcut paths as-is.
        paths.retain(|path| path.is_absolute() && path.exists());
        return Ok(paths);
    }
    let count = unsafe { shell_windows.Count()? };

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

        let current_paths = match selected_paths(&folder_view) {
            Ok(paths) => paths,
            Err(_) => continue,
        };

        if current_paths.is_empty() {
            continue;
        }

        if browser_hwnd == foreground_window {
            if active_tab.0.is_null() {
                return Ok(current_paths);
            }

            if let Some(shell_browser_window) = get_shell_browser_window(&browser)
                && shell_browser_window == active_tab
            {
                return Ok(current_paths);
            }

            if foreground_fallback_paths.is_empty() {
                foreground_fallback_paths = current_paths;
            }

            continue;
        }
    }

    if !foreground_fallback_paths.is_empty() {
        return Ok(foreground_fallback_paths);
    }

    Ok(Vec::new())
}

#[cfg(target_os = "windows")]
fn is_desktop_window(hwnd: HWND) -> bool {
    let mut name = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut name) };
    matches!(
        String::from_utf16_lossy(&name[..len.max(0) as usize]).as_str(),
        "Progman" | "WorkerW"
    )
}

#[cfg(target_os = "windows")]
fn desktop_folder_view(shell_windows: &IShellWindows) -> anyhow::Result<IShellFolderViewDual> {
    let location = VARIANT::from(windows::Win32::UI::Shell::CSIDL_DESKTOP as i32);
    let root = VARIANT::default();
    let mut hwnd = 0;
    let dispatch = unsafe {
        shell_windows.FindWindowSW(&location, &root, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)
    }?;
    let service: IServiceProvider = dispatch.cast()?;
    let browser: IShellBrowser = unsafe { service.QueryService(&SID_STopLevelBrowser) }?;
    let view = unsafe { browser.QueryActiveShellView() }?;
    // The background object exposes IDispatch first; the folder automation
    // interface lives on that object rather than directly on IShellView.
    let dispatch: windows::Win32::System::Com::IDispatch =
        unsafe { view.GetItemObject(SVGIO_BACKGROUND) }?;
    Ok(dispatch.cast()?)
}

#[cfg(target_os = "windows")]
fn selected_paths(folder_view: &IShellFolderViewDual) -> anyhow::Result<Vec<PathBuf>> {
    let items = unsafe { folder_view.SelectedItems() }?;
    let count = unsafe { items.Count() }?;
    let mut paths = Vec::new();
    for ix in 0..count {
        let Ok(item) = (unsafe { items.Item(&VARIANT::from(ix)) }) else {
            continue;
        };
        let Ok(path) = (unsafe { item.Path() }) else {
            continue;
        };
        let path = path.to_string();
        if !path.trim().is_empty() {
            paths.push(PathBuf::from(path));
        }
    }
    Ok(paths)
}

#[cfg(all(test, target_os = "windows"))]
mod windows_tests;

#[cfg(target_os = "macos")]
fn selected_files_macos() -> anyhow::Result<Vec<PathBuf>> {
    let script = r#"
        tell application "System Events" to set frontApp to name of first application process whose frontmost is true
        if frontApp is not "Finder" then return ""
        tell application "Finder"
            set selectedItems to selection
            if (count of selectedItems) is 0 then return ""
            set outputText to ""
            repeat with anItem in selectedItems
                set outputText to outputText & POSIX path of (anItem as alias) & linefeed
            end repeat
            return outputText
        end tell
    "#;

    let output_text = unsafe {
        let pool = NSAutoreleasePool::new(cocoa::base::nil);

        let source = NSString::alloc(cocoa::base::nil).init_str(script);
        let apple_script: *mut Object = msg_send![class!(NSAppleScript), alloc];
        let apple_script: *mut Object = msg_send![apple_script, initWithSource: source];

        if apple_script.is_null() {
            let _: () = msg_send![pool, drain];
            return Ok(Vec::new());
        }

        let mut error: *mut Object = ptr::null_mut();
        let descriptor: *mut Object = msg_send![apple_script, executeAndReturnError: &mut error];
        if descriptor.is_null() {
            let _: () = msg_send![pool, drain];
            return Ok(Vec::new());
        }

        let ns_string: *mut Object = msg_send![descriptor, stringValue];
        if ns_string.is_null() {
            let _: () = msg_send![pool, drain];
            return Ok(Vec::new());
        }

        let utf8_ptr: *const c_char = msg_send![ns_string, UTF8String];
        if utf8_ptr.is_null() {
            let _: () = msg_send![pool, drain];
            return Ok(Vec::new());
        }

        let value = CStr::from_ptr(utf8_ptr).to_string_lossy().into_owned();
        let _: () = msg_send![pool, drain];
        value
    };

    let files = output_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect::<Vec<_>>();

    Ok(files)
}
