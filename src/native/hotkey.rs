//! Global shortcut configuration and narrow OS keyboard adapters.
mod bindings;
mod matcher;
use crate::ipc::IpcEvent;
use bindings::{ALT, CTRL, SHIFT};
pub use bindings::{bindings, configure, default_bindings, save_bindings};
use matcher::{Key, Matcher, Outcome};
use std::sync::mpsc::Sender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkeyAction {
    Show,
    ToggleClipboardChooser,
    ToggleFilePreview,
    TriggerSelectionToCommand,
    DismissEscapeTarget,
}
impl HotkeyAction {
    fn from_slot(slot: usize) -> Self {
        [
            Self::Show,
            Self::ToggleClipboardChooser,
            Self::ToggleFilePreview,
            Self::TriggerSelectionToCommand,
        ][slot]
    }
    fn into_ipc_event(self, focused_window: Option<u64>) -> IpcEvent {
        match self {
            Self::Show => IpcEvent::Show,
            Self::ToggleClipboardChooser => IpcEvent::ToggleClipboardChooser { focused_window },
            Self::ToggleFilePreview => IpcEvent::ToggleFilePreview,
            Self::TriggerSelectionToCommand => IpcEvent::TriggerSelectionToCommand,
            Self::DismissEscapeTarget => IpcEvent::DismissEscapeTarget,
        }
    }
}
pub fn start_global_hotkey_listener(tx: Sender<IpcEvent>) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("global-hotkey-listener".into())
        .spawn(move || {
            if let Err(error) = start_listen(tx) {
                tracing::error!(%error,"Global hotkey listener stopped");
            }
        })?;
    Ok(())
}
fn dispatch(tx: &Sender<IpcEvent>, outcome: Outcome, focused: Option<u64>) {
    if let Some(action) = outcome.action {
        let _ = tx.send(action.into_ipc_event(focused));
    }
}
#[cfg(target_os = "windows")]
struct WindowsContext {
    tx: Sender<IpcEvent>,
    matcher: Matcher,
}
#[cfg(target_os = "windows")]
static WINDOWS_CONTEXT: std::sync::OnceLock<std::sync::Mutex<WindowsContext>> =
    std::sync::OnceLock::new();
#[cfg(target_os = "windows")]
fn windows_key(code: u32) -> Key {
    match code {
        0x11 | 0xa2 => Key::Modifier(CTRL, 1),
        0xa3 => Key::Modifier(CTRL, 2),
        0x12 | 0xa4 => Key::Modifier(ALT, 1),
        0xa5 => Key::Modifier(ALT, 2),
        0x10 | 0xa0 => Key::Modifier(SHIFT, 1),
        0xa1 => Key::Modifier(SHIFT, 2),
        // Meta is not supported syntax; it must consume modifier-only gestures.
        0x5b => Key::Modifier(8, 1),
        0x5c => Key::Modifier(8, 2),
        code if code < 256 => Key::Code(code as u16),
        _ => Key::Other,
    }
}
#[cfg(target_os = "windows")]
unsafe extern "system" fn keyboard_hook(
    code: i32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetForegroundWindow, GetWindowThreadProcessId, KBDLLHOOKSTRUCT, WM_KEYDOWN,
        WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };
    if code >= 0
        && let Some(context) = WINDOWS_CONTEXT.get()
    {
        let message = wparam as u32;
        let pressed = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
        if pressed || message == WM_KEYUP || message == WM_SYSKEYUP {
            let event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            let foreground = unsafe { GetForegroundWindow() };
            let mut pid = 0;
            if !foreground.is_null() {
                unsafe {
                    GetWindowThreadProcessId(foreground, &mut pid);
                }
            }
            let mut context = context.lock().unwrap_or_else(|e| e.into_inner());
            let outcome = if event.vkCode == 0x1b {
                context.matcher.escape(
                    pressed,
                    pid == std::process::id()
                        && crate::native::windows::has_escape_dismiss_target(),
                )
            } else {
                context
                    .matcher
                    .event(windows_key(event.vkCode), pressed, bindings::snapshot())
            };
            let suppress = outcome.suppress;
            dispatch(
                &context.tx,
                outcome,
                (!foreground.is_null()).then_some(foreground as u64),
            );
            if suppress {
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}
#[cfg(target_os = "windows")]
mod windows_listener;
#[cfg(target_os = "windows")]
fn start_listen(tx: Sender<IpcEvent>) -> anyhow::Result<()> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG};
    WINDOWS_CONTEXT
        .set(std::sync::Mutex::new(WindowsContext {
            tx,
            matcher: Matcher::default(),
        }))
        .map_err(|_| anyhow::anyhow!("Keyboard listener already started"))?;
    let mut listener = windows_listener::Listener::new()?;
    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if result == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        if result == 0 {
            return Ok(());
        }
        listener.process(&message);
    }
}
#[cfg(target_os = "macos")]
fn mac_key(code: i64) -> Key {
    let key = match code {
        0 => 65,
        1 => 83,
        2 => 68,
        3 => 70,
        4 => 72,
        5 => 71,
        6 => 90,
        7 => 88,
        8 => 67,
        9 => 86,
        11 => 66,
        12 => 81,
        13 => 87,
        14 => 69,
        15 => 82,
        16 => 89,
        17 => 84,
        18 => 49,
        19 => 50,
        20 => 51,
        21 => 52,
        22 => 54,
        23 => 53,
        25 => 57,
        26 => 55,
        28 => 56,
        29 => 48,
        31 => 79,
        32 => 85,
        34 => 73,
        35 => 80,
        37 => 76,
        38 => 74,
        40 => 75,
        45 => 78,
        46 => 77,
        49 => 32,
        122 => 112,
        120 => 113,
        99 => 114,
        118 => 115,
        96 => 116,
        97 => 117,
        98 => 118,
        100 => 119,
        101 => 120,
        109 => 121,
        103 => 122,
        111 => 123,
        _ => return Key::Other,
    };
    Key::Code(key)
}
#[cfg(target_os = "macos")]
fn start_listen(tx: Sender<IpcEvent>) -> anyhow::Result<()> {
    use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
    use core_graphics::event::{
        CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
        CGEventType, EventField,
    };
    let matcher = std::sync::Mutex::new(Matcher::default());
    let tap = CGEventTap::new(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![
            CGEventType::FlagsChanged,
            CGEventType::KeyDown,
            CGEventType::KeyUp,
        ],
        move |_, kind, event| {
            let flags = event.get_flags();
            let modifiers = if flags.intersects(CGEventFlags::CGEventFlagCommand)
                || flags.intersects(CGEventFlags::CGEventFlagControl)
            {
                CTRL
            } else {
                0
            } | if flags.intersects(CGEventFlags::CGEventFlagAlternate) {
                ALT
            } else {
                0
            } | if flags.intersects(CGEventFlags::CGEventFlagShift) {
                SHIFT
            } else {
                0
            };
            let snapshot = bindings::snapshot();
            let mut matcher = matcher.lock().unwrap_or_else(|e| e.into_inner());
            let flags_outcome = matcher.flags(modifiers, snapshot);
            dispatch(&tx, flags_outcome, None);
            let is_key_down = matches!(kind, CGEventType::KeyDown);
            if matches!(kind, CGEventType::KeyDown | CGEventType::KeyUp) {
                let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
                let outcome = if code == 0x35 {
                    matcher.escape(
                        is_key_down,
                        crate::native::windows::has_escape_dismiss_target(),
                    )
                } else {
                    matcher.event(mac_key(code), is_key_down, snapshot)
                };
                let suppress = outcome.suppress;
                dispatch(&tx, outcome, None);
                if suppress {
                    event.set_type(CGEventType::Null);
                }
            }
            None
        },
    )
    .map_err(|_| {
        anyhow::anyhow!(
            "Enable Accessibility for Rustickers in System Settings > Privacy & Security"
        )
    })?;
    let source = tap
        .mach_port
        .create_runloop_source(0)
        .map_err(|_| anyhow::anyhow!("Could not create keyboard run-loop source"))?;
    unsafe {
        CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes);
        tap.enable();
        CFRunLoop::run_current();
    }
    Ok(())
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn start_listen(tx: Sender<IpcEvent>) -> anyhow::Result<()> {
    use rdev::{EventType, Key as RKey};
    fn map_key(key: RKey) -> Key {
        match key {
            RKey::ControlLeft => Key::Modifier(CTRL, 1),
            RKey::ControlRight => Key::Modifier(CTRL, 2),
            RKey::Alt => Key::Modifier(ALT, 1),
            RKey::AltGr => Key::Modifier(ALT, 2),
            RKey::ShiftLeft => Key::Modifier(SHIFT, 1),
            RKey::ShiftRight => Key::Modifier(SHIFT, 2),
            RKey::MetaLeft => Key::Modifier(8, 1),
            RKey::MetaRight => Key::Modifier(8, 2),
            RKey::Space => Key::Code(32),
            RKey::KeyA => Key::Code(65),
            RKey::KeyB => Key::Code(66),
            RKey::KeyC => Key::Code(67),
            RKey::KeyD => Key::Code(68),
            RKey::KeyE => Key::Code(69),
            RKey::KeyF => Key::Code(70),
            RKey::KeyG => Key::Code(71),
            RKey::KeyH => Key::Code(72),
            RKey::KeyI => Key::Code(73),
            RKey::KeyJ => Key::Code(74),
            RKey::KeyK => Key::Code(75),
            RKey::KeyL => Key::Code(76),
            RKey::KeyM => Key::Code(77),
            RKey::KeyN => Key::Code(78),
            RKey::KeyO => Key::Code(79),
            RKey::KeyP => Key::Code(80),
            RKey::KeyQ => Key::Code(81),
            RKey::KeyR => Key::Code(82),
            RKey::KeyS => Key::Code(83),
            RKey::KeyT => Key::Code(84),
            RKey::KeyU => Key::Code(85),
            RKey::KeyV => Key::Code(86),
            RKey::KeyW => Key::Code(87),
            RKey::KeyX => Key::Code(88),
            RKey::KeyY => Key::Code(89),
            RKey::KeyZ => Key::Code(90),
            RKey::Num0 => Key::Code(48),
            RKey::Num1 => Key::Code(49),
            RKey::Num2 => Key::Code(50),
            RKey::Num3 => Key::Code(51),
            RKey::Num4 => Key::Code(52),
            RKey::Num5 => Key::Code(53),
            RKey::Num6 => Key::Code(54),
            RKey::Num7 => Key::Code(55),
            RKey::Num8 => Key::Code(56),
            RKey::Num9 => Key::Code(57),
            RKey::F1 => Key::Code(112),
            RKey::F2 => Key::Code(113),
            RKey::F3 => Key::Code(114),
            RKey::F4 => Key::Code(115),
            RKey::F5 => Key::Code(116),
            RKey::F6 => Key::Code(117),
            RKey::F7 => Key::Code(118),
            RKey::F8 => Key::Code(119),
            RKey::F9 => Key::Code(120),
            RKey::F10 => Key::Code(121),
            RKey::F11 => Key::Code(122),
            RKey::F12 => Key::Code(123),
            _ => Key::Other,
        }
    }
    let state = std::sync::Mutex::new(Matcher::default());
    rdev::listen(move |event| {
        let (key, pressed) = match event.event_type {
            EventType::KeyPress(key) => (key, true),
            EventType::KeyRelease(key) => (key, false),
            _ => return,
        };
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        let outcome = if key == RKey::Escape {
            state.escape(pressed, crate::native::windows::has_escape_dismiss_target())
        } else {
            state.event(map_key(key), pressed, bindings::snapshot())
        };
        dispatch(&tx, outcome, None);
    })
    .map_err(|error| anyhow::anyhow!("Keyboard listener failed: {error:?}"))
}
