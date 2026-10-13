//! Cheap, conservative empty-selection checks; unknown apps still use Copy.
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
    ]
    .iter()
    .any(|known| class.eq_ignore_ascii_case(known))
}

pub(super) fn known_empty_selection(source: Option<u64>) -> bool {
    let Some(source) = source else {
        return true; // No foreground application to copy from.
    };
    let hwnd = source as HWND;
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
    // Desktop shell windows cannot supply a text selection. Do not classify
    // Explorer's file windows, consoles, browsers, or missing caret information
    // as empty: many real selections are not represented by a Win32 caret.
    matches!(class_name(hwnd).as_str(), "Progman" | "WorkerW")
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
        assert!(!native_text_class("RichEditCustomWidget"));
        assert!(!native_text_class("Chrome_WidgetWin_1"));
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
