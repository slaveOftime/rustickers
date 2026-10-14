//! Opt-in desktop Shell integration using disposable files and restored selection.
use super::*;
use std::time::{Duration, Instant};
use windows::{
    Win32::{
        System::Com::IDispatch,
        UI::{Shell::FolderItem, WindowsAndMessaging::GetShellWindow},
    },
    core::BSTR,
};

fn selected_variants(view: &IShellFolderViewDual) -> anyhow::Result<Vec<VARIANT>> {
    let items = unsafe { view.SelectedItems() }?;
    (0..unsafe { items.Count() }?)
        .map(|ix| {
            let item = unsafe { items.Item(&VARIANT::from(ix)) }?;
            let dispatch: IDispatch = item.cast()?;
            Ok(VARIANT::from(dispatch))
        })
        .collect()
}
fn deselect(view: &IShellFolderViewDual) -> anyhow::Result<()> {
    for item in selected_variants(view)? {
        unsafe { view.SelectItem(&item, 0) }?;
    }
    Ok(())
}
struct RestoreDesktop {
    view: IShellFolderViewDual,
    selected: Vec<VARIANT>,
}
impl Drop for RestoreDesktop {
    fn drop(&mut self) {
        let _ = deselect(&self.view);
        for item in &self.selected {
            let _ = unsafe { self.view.SelectItem(item, 1) };
        }
    }
}
struct ComGuard;
impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

#[test]
#[ignore = "opt-in Windows Shell test: temporarily creates/selects desktop text files; restores selection"]
fn reads_selected_desktop_files_without_an_explorer_window() -> anyhow::Result<()> {
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() }?;
    let _com = ComGuard;
    let windows: IShellWindows = unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_ALL) }?;
    let view = desktop_folder_view(&windows)?;
    let original_selection = selected_variants(&view)?;
    let user_dirs =
        directories::UserDirs::new().ok_or_else(|| anyhow::anyhow!("No user directories"))?;
    let desktop_dir = user_dirs
        .desktop_dir()
        .ok_or_else(|| anyhow::anyhow!("No desktop directory"))?;
    let first = tempfile::Builder::new()
        .prefix("rustickers-preview-test-")
        .suffix(".txt")
        .tempfile_in(desktop_dir)?;
    let second = tempfile::Builder::new()
        .prefix("rustickers-preview-test-")
        .suffix(".txt")
        .tempfile_in(desktop_dir)?;
    std::fs::write(first.path(), "isolated desktop preview fixture one")?;
    std::fs::write(second.path(), "isolated desktop preview fixture two")?;
    let folder = unsafe { view.Folder() }?;
    let mut entries: Vec<FolderItem> = Vec::new();
    for file in [&first, &second] {
        let name = BSTR::from(file.path().file_name().unwrap().to_string_lossy().as_ref());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(item) = unsafe { folder.ParseName(&name) } {
                entries.push(item);
                break;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "Shell did not discover test file"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    let desktop = unsafe { GetShellWindow() };
    anyhow::ensure!(!desktop.0.is_null(), "No shell desktop window");
    // Supply the actual desktop HWND at the foreground-resolution seam;
    // Windows need not allow this test runner to steal foreground focus.
    anyhow::ensure!(is_desktop_window(desktop));
    // Declared after the files so selection is restored before they disappear,
    // including when an assertion below fails.
    let restore = RestoreDesktop {
        selected: original_selection,
        view: view.clone(),
    };
    deselect(&view)?;
    anyhow::ensure!(
        selected_files_windows_for_window(desktop)?.is_empty(),
        "Empty desktop must not read an Explorer selection"
    );
    for (ix, entry) in entries.iter().enumerate() {
        let dispatch: IDispatch = entry.cast()?;
        let item = VARIANT::from(dispatch);
        let mut expected: Vec<PathBuf> = [&first, &second]
            .into_iter()
            .take(ix + 1)
            .map(|f| f.path().to_owned())
            .collect();
        expected.sort();
        // ParseName can find a newly created item before the visual desktop
        // has processed its directory-change notification. Retry selection.
        let deadline = Instant::now() + Duration::from_secs(5);
        let actual = loop {
            unsafe { view.SelectItem(&item, 1) }?;
            let mut actual = selected_files_windows_for_window(desktop)?;
            actual.sort();
            if actual == expected {
                break actual;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "Wrong desktop selection: {actual:?}, expected {expected:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        eprintln!(
            "Resolved {} selected desktop file(s) without opening Explorer",
            actual.len()
        );
    }
    // Restore selected items before the temporary files disappear.
    drop(restore);
    Ok(())
}
