//! Native empty-selection checks and terminal-safe Copy policy.
use windows_sys::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        GUITHREADINFO, GetClassNameW, GetGUIThreadInfo, GetWindowThreadProcessId, SMTO_ABORTIFHUNG,
        SMTO_BLOCK, SendMessageTimeoutW,
    },
};

fn class_name(hwnd: HWND) -> String {
    let mut name = [0; 256];
    let len = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
    String::from_utf16_lossy(&name[..len.max(0) as usize])
}

fn native_text_class(class: &str) -> bool {
    [
        "Edit",
        "RichEdit",
        "RichEdit20A",
        "RichEdit20W",
        "RICHEDIT50W",
        // Windows 11 Notepad's Microsoft 365 RichEdit plain-text canvas:
        // https://devblogs.microsoft.com/math-in-office/windows-11-notepad/
        "RichEditD2DPT",
    ]
    .iter()
    .any(|known| class.eq_ignore_ascii_case(known))
}

pub(super) fn known_empty_selection(source: Option<u64>) -> bool {
    let Some(source) = source else {
        return true; // No foreground application to copy from.
    };
    let hwnd = source as HWND;
    if matches!(class_name(hwnd).as_str(), "Progman" | "WorkerW") {
        return true;
    }
    let thread = unsafe { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) };
    if thread == 0 {
        return false; // Invalid/stale handle: let the normal focus check reject it.
    }
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetGUIThreadInfo(thread, &mut info) } == 0 {
        return false;
    }
    let focus_class = class_name(info.hwndFocus);
    if native_text_class(&focus_class) {
        // EM_GETSEL is marshalled by USER32. Avoid remote-process pointers and
        // use its packed return; -1 (selection beyond 64K) means unknown, never
        // empty. A hung/unsupported/protected control must not block the popup.
        const EM_GETSEL: u32 = 0x00b0;
        let mut selection = 0;
        let ok = unsafe {
            SendMessageTimeoutW(
                info.hwndFocus,
                EM_GETSEL,
                0,
                0,
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                30,
                &mut selection,
            )
        } != 0;
        return ok && packed_selection_is_empty(selection);
    }
    // Missing caret information alone does not prove an empty selection.
    false
}

pub(super) fn copy_is_unsafe(source: u64) -> bool {
    terminal_class(&class_name(source as HWND))
}

fn terminal_class(class: &str) -> bool {
    // With no selection, Ctrl+C is SIGINT/break, not a harmless Copy attempt.
    // UI Automation may read their selection, but must never fall back to keys.
    matches!(
        class,
        "ConsoleWindowClass"
            | "CASCADIA_HOSTING_WINDOW_CLASS"
            | "mintty"
            | "PuTTY"
            | "VirtualConsoleClass"
            | "VirtualConsoleClassMain"
    )
}

fn packed_selection_is_empty(selection: usize) -> bool {
    let selection = selection as u32;
    selection != u32::MAX && selection & 0xffff == selection >> 16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_native_text_classes_are_probed() {
        assert!(native_text_class("Edit"));
        assert!(native_text_class("RichEdit20W"));
        assert!(native_text_class("RICHEDIT50W"));
        assert!(native_text_class("RichEDitD2DPT"));
        assert!(!native_text_class("RichEditCustomWidget"));
        assert!(!native_text_class("Chrome_WidgetWin_1"));
    }

    #[test]
    fn terminal_windows_must_never_receive_a_blind_copy() {
        for class in [
            "ConsoleWindowClass",
            "CASCADIA_HOSTING_WINDOW_CLASS",
            "mintty",
            "PuTTY",
            "VirtualConsoleClass",
            "VirtualConsoleClassMain",
        ] {
            assert!(terminal_class(class));
        }
        assert!(!terminal_class("Chrome_WidgetWin_1"));
        assert!(!terminal_class("Notepad"));
    }

    #[test]
    fn packed_selection_only_proves_empty_with_equal_valid_offsets() {
        assert!(packed_selection_is_empty(0));
        assert!(packed_selection_is_empty(42 | (42 << 16)));
        assert!(!packed_selection_is_empty(42 | (43 << 16)));
        assert!(!packed_selection_is_empty(u32::MAX as usize));
        assert!(!packed_selection_is_empty(usize::MAX));
    }
}
