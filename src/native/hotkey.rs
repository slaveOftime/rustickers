use std::sync::mpsc::Sender;

use crate::ipc::IpcEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkeyAction {
    Show,
    ToggleFilePreview,
    TriggerSelectionToCommand,
    DismissEscapeTarget,
}

impl HotkeyAction {
    fn into_ipc_event(self) -> IpcEvent {
        match self {
            Self::Show => IpcEvent::Show,
            Self::ToggleFilePreview => IpcEvent::ToggleFilePreview,
            Self::TriggerSelectionToCommand => IpcEvent::TriggerSelectionToCommand,
            Self::DismissEscapeTarget => IpcEvent::DismissEscapeTarget,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkeyKey {
    CtrlLeft,
    CtrlRight,
    AltLeft,
    AltRight,
    R,
    Space,
    Other,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
struct HotkeyOutcome {
    action: Option<HotkeyAction>,
    suppress: bool,
}

#[derive(Default, Debug, Clone, Copy)]
struct KeyState {
    ctrl_left: bool,
    ctrl_right: bool,
    alt_left: bool,
    alt_right: bool,
    r_down: bool,
    space_down: bool,
    suppress_r_until_release: bool,
    suppress_space_until_release: bool,
    suppress_escape_until_release: bool,
    modifier_combo_active: bool,
    modifier_combo_consumed: bool,
}

impl KeyState {
    fn ctrl(self) -> bool {
        self.ctrl_left || self.ctrl_right
    }

    fn alt(self) -> bool {
        self.alt_left || self.alt_right
    }

    fn key_down(&mut self, key: HotkeyKey) -> HotkeyOutcome {
        match key {
            HotkeyKey::CtrlLeft => self.ctrl_left = true,
            HotkeyKey::CtrlRight => self.ctrl_right = true,
            HotkeyKey::AltLeft => self.alt_left = true,
            HotkeyKey::AltRight => self.alt_right = true,
            HotkeyKey::R => {
                if !self.r_down {
                    self.r_down = true;
                    if self.ctrl() && self.alt() {
                        self.modifier_combo_consumed = true;
                        self.suppress_r_until_release = true;
                        return HotkeyOutcome {
                            action: Some(HotkeyAction::Show),
                            suppress: true,
                        };
                    }
                }
                if self.modifier_combo_active {
                    self.modifier_combo_consumed = true;
                }
                return HotkeyOutcome {
                    suppress: self.suppress_r_until_release,
                    ..HotkeyOutcome::default()
                };
            }
            HotkeyKey::Space => {
                if !self.space_down {
                    self.space_down = true;
                    if self.ctrl() && !self.alt() {
                        self.suppress_space_until_release = true;
                        return HotkeyOutcome {
                            action: Some(HotkeyAction::TriggerSelectionToCommand),
                            suppress: true,
                        };
                    }
                }
                if self.modifier_combo_active {
                    self.modifier_combo_consumed = true;
                }
                return HotkeyOutcome {
                    suppress: self.suppress_space_until_release,
                    ..HotkeyOutcome::default()
                };
            }
            HotkeyKey::Other => {
                if self.modifier_combo_active {
                    self.modifier_combo_consumed = true;
                }
            }
        }

        if self.ctrl() && self.alt() && !self.modifier_combo_active {
            self.modifier_combo_active = true;
            self.modifier_combo_consumed = false;
        }
        HotkeyOutcome::default()
    }

    fn key_up(&mut self, key: HotkeyKey) -> HotkeyOutcome {
        let mut outcome = HotkeyOutcome::default();
        match key {
            HotkeyKey::CtrlLeft => self.ctrl_left = false,
            HotkeyKey::CtrlRight => self.ctrl_right = false,
            HotkeyKey::AltLeft => self.alt_left = false,
            HotkeyKey::AltRight => self.alt_right = false,
            HotkeyKey::R => {
                self.r_down = false;
                outcome.suppress = self.suppress_r_until_release;
                self.suppress_r_until_release = false;
            }
            HotkeyKey::Space => {
                self.space_down = false;
                outcome.suppress = self.suppress_space_until_release;
                self.suppress_space_until_release = false;
            }
            HotkeyKey::Other => {}
        }
        if self.modifier_combo_active && !self.ctrl() && !self.alt() {
            if !self.modifier_combo_consumed {
                outcome.action = Some(HotkeyAction::ToggleFilePreview);
            }
            self.modifier_combo_active = false;
            self.modifier_combo_consumed = false;
        }
        outcome
    }

    fn escape_down(&mut self, has_escape_target: bool) -> HotkeyOutcome {
        if self.suppress_escape_until_release {
            return HotkeyOutcome {
                suppress: true,
                ..HotkeyOutcome::default()
            };
        }
        if has_escape_target {
            self.suppress_escape_until_release = true;
            return HotkeyOutcome {
                action: Some(HotkeyAction::DismissEscapeTarget),
                suppress: true,
            };
        }
        HotkeyOutcome::default()
    }

    fn escape_up(&mut self) -> HotkeyOutcome {
        let suppress = self.suppress_escape_until_release;
        self.suppress_escape_until_release = false;
        HotkeyOutcome {
            suppress,
            ..HotkeyOutcome::default()
        }
    }
}

pub fn start_global_hotkey_listener(ipc_events_tx: Sender<IpcEvent>) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("global-hotkey-listener".to_string())
        .spawn(move || {
            tracing::info!("Global hotkey listener started");
            if let Err(err) = start_listen(ipc_events_tx) {
                tracing::error!(error = %err, "Global hotkey listener stopped");
            }
        })?;
    Ok(())
}

fn dispatch_action(ipc_events_tx: &Sender<IpcEvent>, action: Option<HotkeyAction>) {
    let Some(action) = action else { return };
    tracing::debug!(?action, "Global hotkey triggered");
    let _ = ipc_events_tx.send(action.into_ipc_event());
}

#[cfg(target_os = "windows")]
struct WindowsHotkeyContext {
    ipc_events_tx: Sender<IpcEvent>,
    state: KeyState,
}

#[cfg(target_os = "windows")]
static WINDOWS_HOTKEY_CONTEXT: std::sync::OnceLock<std::sync::Mutex<WindowsHotkeyContext>> =
    std::sync::OnceLock::new();

#[cfg(target_os = "windows")]
fn windows_hotkey_key(vk_code: u32) -> HotkeyKey {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_MENU, VK_RCONTROL, VK_RMENU, VK_SPACE,
    };

    const VK_R: u32 = b'R' as u32;

    match vk_code {
        code if code == VK_LCONTROL as u32 || code == VK_CONTROL as u32 => HotkeyKey::CtrlLeft,
        code if code == VK_RCONTROL as u32 => HotkeyKey::CtrlRight,
        code if code == VK_LMENU as u32 || code == VK_MENU as u32 => HotkeyKey::AltLeft,
        code if code == VK_RMENU as u32 => HotkeyKey::AltRight,
        VK_R => HotkeyKey::R,
        code if code == VK_SPACE as u32 => HotkeyKey::Space,
        _ => HotkeyKey::Other,
    }
}

#[cfg(target_os = "windows")]
fn is_windows_escape(vk_code: u32) -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;

    vk_code == VK_ESCAPE as u32
}

#[cfg(target_os = "windows")]
fn is_rustickers_foreground() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };

    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return false;
    }
    let mut process_id = 0;
    unsafe {
        GetWindowThreadProcessId(foreground, &mut process_id);
    }
    process_id == std::process::id()
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn windows_keyboard_hook(
    code: i32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    if code >= 0 {
        let message = wparam as u32;
        let is_key_down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
        let is_key_up = message == WM_KEYUP || message == WM_SYSKEYUP;

        if is_key_down || is_key_up {
            let keyboard_event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            if let Some(context) = WINDOWS_HOTKEY_CONTEXT.get() {
                let (ipc_events_tx, outcome) = {
                    let mut context = match context.lock() {
                        Ok(context) => context,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    let outcome = if is_windows_escape(keyboard_event.vkCode) {
                        if is_key_down {
                            context.state.escape_down(
                                is_rustickers_foreground()
                                    && crate::native::windows::has_escape_dismiss_target(),
                            )
                        } else {
                            context.state.escape_up()
                        }
                    } else if is_key_down {
                        context
                            .state
                            .key_down(windows_hotkey_key(keyboard_event.vkCode))
                    } else {
                        context
                            .state
                            .key_up(windows_hotkey_key(keyboard_event.vkCode))
                    };
                    (context.ipc_events_tx.clone(), outcome)
                };

                dispatch_action(&ipc_events_tx, outcome.action);
                if outcome.suppress {
                    return 1;
                }
            }
        }
    }

    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

#[cfg(target_os = "windows")]
fn start_listen(ipc_events_tx: Sender<IpcEvent>) -> anyhow::Result<()> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetMessageW, MSG, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    };

    WINDOWS_HOTKEY_CONTEXT
        .set(std::sync::Mutex::new(WindowsHotkeyContext {
            ipc_events_tx,
            state: KeyState::default(),
        }))
        .map_err(|_| anyhow::anyhow!("Windows hotkey listener was already started"))?;

    let hook = unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(windows_keyboard_hook),
            std::ptr::null_mut(),
            0,
        )
    };
    if hook.is_null() {
        return Err(anyhow::anyhow!(
            "failed to install Windows keyboard hook: {}",
            std::io::Error::last_os_error()
        ));
    }

    let mut message = MSG::default();
    let result = loop {
        let result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if result <= 0 {
            break result;
        }
    };
    unsafe {
        UnhookWindowsHookEx(hook);
    }

    if result == -1 {
        Err(anyhow::anyhow!(
            "Windows hotkey message loop failed: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn start_listen(ipc_events_tx: Sender<IpcEvent>) -> anyhow::Result<()> {
    use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
    use core_graphics::event::{
        CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
        CGEventType, EventField,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    const KEY_R: i64 = 0x0f;
    const KEY_SPACE: i64 = 0x31;
    const KEY_ESCAPE: i64 = 0x35;

    let modifier_combo_down = Arc::new(AtomicBool::new(false));
    let modifier_combo_down_for_tap = modifier_combo_down.clone();
    let modifier_combo_consumed = Arc::new(AtomicBool::new(false));
    let modifier_combo_consumed_for_tap = modifier_combo_consumed.clone();
    let suppress_r_until_release = Arc::new(AtomicBool::new(false));
    let suppress_r_until_release_for_tap = suppress_r_until_release.clone();
    let suppress_space_until_release = Arc::new(AtomicBool::new(false));
    let suppress_space_until_release_for_tap = suppress_space_until_release.clone();
    let suppress_escape_until_release = Arc::new(AtomicBool::new(false));
    let suppress_escape_until_release_for_tap = suppress_escape_until_release.clone();
    let tap = CGEventTap::new(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![
            CGEventType::FlagsChanged,
            CGEventType::KeyDown,
            CGEventType::KeyUp,
        ],
        move |_, event_type, event| {
            let flags = event.get_flags();
            let primary = flags.intersects(CGEventFlags::CGEventFlagCommand)
                || flags.intersects(CGEventFlags::CGEventFlagControl);
            let alt = flags.intersects(CGEventFlags::CGEventFlagAlternate);
            let key_code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            let is_repeat =
                event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;
            let modifier_combo = primary && alt;
            let any_combo_modifier = primary || alt;
            let modifier_combo_was_down = modifier_combo_down_for_tap.load(Ordering::Relaxed);

            match event_type {
                CGEventType::FlagsChanged if modifier_combo && !modifier_combo_was_down => {
                    modifier_combo_down_for_tap.store(true, Ordering::Relaxed);
                    modifier_combo_consumed_for_tap.store(false, Ordering::Relaxed);
                }
                CGEventType::FlagsChanged if !any_combo_modifier && modifier_combo_was_down => {
                    modifier_combo_down_for_tap.store(false, Ordering::Relaxed);
                    if !modifier_combo_consumed_for_tap.swap(false, Ordering::Relaxed) {
                        dispatch_action(&ipc_events_tx, Some(HotkeyAction::ToggleFilePreview));
                    }
                }
                CGEventType::KeyDown if primary && alt && key_code == KEY_R => {
                    modifier_combo_consumed_for_tap.store(true, Ordering::Relaxed);
                    suppress_r_until_release_for_tap.store(true, Ordering::Relaxed);
                    if !is_repeat {
                        dispatch_action(&ipc_events_tx, Some(HotkeyAction::Show));
                    }
                    event.set_type(CGEventType::Null);
                }
                CGEventType::KeyUp
                    if key_code == KEY_R
                        && suppress_r_until_release_for_tap.swap(false, Ordering::Relaxed) =>
                {
                    event.set_type(CGEventType::Null);
                }
                CGEventType::KeyDown if primary && !alt && key_code == KEY_SPACE => {
                    suppress_space_until_release_for_tap.store(true, Ordering::Relaxed);
                    if !is_repeat {
                        dispatch_action(
                            &ipc_events_tx,
                            Some(HotkeyAction::TriggerSelectionToCommand),
                        );
                    }
                    event.set_type(CGEventType::Null);
                }
                CGEventType::KeyUp
                    if key_code == KEY_SPACE
                        && suppress_space_until_release_for_tap.swap(false, Ordering::Relaxed) =>
                {
                    event.set_type(CGEventType::Null);
                }
                CGEventType::KeyDown
                    if key_code == KEY_ESCAPE
                        && (crate::native::windows::has_escape_dismiss_target()
                            || suppress_escape_until_release_for_tap.load(Ordering::Relaxed)) =>
                {
                    if !suppress_escape_until_release_for_tap.swap(true, Ordering::Relaxed) {
                        dispatch_action(
                            &ipc_events_tx,
                            Some(HotkeyAction::DismissEscapeTarget),
                        );
                    }
                    event.set_type(CGEventType::Null);
                }
                CGEventType::KeyUp
                    if key_code == KEY_ESCAPE
                        && suppress_escape_until_release_for_tap.swap(false, Ordering::Relaxed) =>
                {
                    event.set_type(CGEventType::Null);
                }
                CGEventType::KeyDown if modifier_combo_was_down => {
                    modifier_combo_consumed_for_tap.store(true, Ordering::Relaxed);
                }
                _ => {}
            }

            None
        },
    )
    .map_err(|_| {
        anyhow::anyhow!(
            "failed to create macOS event tap; enable Accessibility for Rustickers in System Settings > Privacy & Security"
        )
    })?;

    let run_loop = CFRunLoop::get_current();
    let source = tap
        .mach_port
        .create_runloop_source(0)
        .map_err(|_| anyhow::anyhow!("failed to create macOS hotkey run-loop source"))?;
    unsafe {
        run_loop.add_source(&source, kCFRunLoopCommonModes);
        tap.enable();
        CFRunLoop::run_current();
    }
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn start_listen(ipc_events_tx: Sender<IpcEvent>) -> anyhow::Result<()> {
    use rdev::{Event, EventType, Key, listen};
    use std::sync::{Arc, Mutex};

    fn map_key(key: Key) -> HotkeyKey {
        match key {
            Key::ControlLeft => HotkeyKey::CtrlLeft,
            Key::ControlRight => HotkeyKey::CtrlRight,
            Key::Alt => HotkeyKey::AltLeft,
            Key::AltGr => HotkeyKey::AltRight,
            Key::KeyR => HotkeyKey::R,
            Key::Space => HotkeyKey::Space,
            _ => HotkeyKey::Other,
        }
    }

    let state = Arc::new(Mutex::new(KeyState::default()));
    let callback = move |event: Event| {
        let outcome = {
            let mut state = match state.lock() {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
            match event.event_type {
                EventType::KeyPress(key) => state.key_down(map_key(key)),
                EventType::KeyRelease(key) => state.key_up(map_key(key)),
                _ => HotkeyOutcome::default(),
            }
        };
        dispatch_action(&ipc_events_tx, outcome.action);
    };

    listen(callback).map_err(|err| anyhow::anyhow!("rdev listen failed: {err:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_space_triggers_once_and_suppresses_repeat_and_release() {
        let mut state = KeyState::default();

        assert_eq!(
            state.key_down(HotkeyKey::CtrlLeft),
            HotkeyOutcome::default()
        );
        assert_eq!(
            state.key_down(HotkeyKey::Space),
            HotkeyOutcome {
                action: Some(HotkeyAction::TriggerSelectionToCommand),
                suppress: true,
            }
        );
        assert_eq!(
            state.key_down(HotkeyKey::Space),
            HotkeyOutcome {
                action: None,
                suppress: true,
            }
        );
        assert_eq!(
            state.key_up(HotkeyKey::Space),
            HotkeyOutcome {
                action: None,
                suppress: true,
            }
        );
    }

    #[test]
    fn consumed_space_release_stays_suppressed_after_ctrl_release() {
        let mut state = KeyState::default();

        state.key_down(HotkeyKey::CtrlLeft);
        state.key_down(HotkeyKey::Space);
        state.key_up(HotkeyKey::CtrlLeft);

        assert!(state.key_up(HotkeyKey::Space).suppress);
    }

    #[test]
    fn overlapping_ctrl_keys_do_not_clear_ctrl_early() {
        let mut state = KeyState::default();

        state.key_down(HotkeyKey::CtrlLeft);
        state.key_down(HotkeyKey::CtrlRight);
        state.key_up(HotkeyKey::CtrlLeft);

        assert_eq!(
            state.key_down(HotkeyKey::Space).action,
            Some(HotkeyAction::TriggerSelectionToCommand)
        );
    }

    #[test]
    fn typing_during_ctrl_alt_does_not_toggle_preview_on_release() {
        let mut state = KeyState::default();

        state.key_down(HotkeyKey::CtrlLeft);
        state.key_down(HotkeyKey::AltLeft);
        state.key_down(HotkeyKey::Other);
        state.key_up(HotkeyKey::AltLeft);

        assert_eq!(state.key_up(HotkeyKey::CtrlLeft).action, None);
    }

    #[test]
    fn bare_ctrl_alt_chord_toggles_preview_once() {
        let mut state = KeyState::default();

        state.key_down(HotkeyKey::CtrlLeft);
        state.key_down(HotkeyKey::AltLeft);
        state.key_up(HotkeyKey::AltLeft);

        assert_eq!(
            state.key_up(HotkeyKey::CtrlLeft).action,
            Some(HotkeyAction::ToggleFilePreview)
        );
    }

    #[test]
    fn ctrl_alt_r_is_suppressed_and_does_not_toggle_preview() {
        let mut state = KeyState::default();

        state.key_down(HotkeyKey::CtrlLeft);
        state.key_down(HotkeyKey::AltLeft);
        assert_eq!(
            state.key_down(HotkeyKey::R),
            HotkeyOutcome {
                action: Some(HotkeyAction::Show),
                suppress: true,
            }
        );
        assert!(state.key_up(HotkeyKey::R).suppress);
        state.key_up(HotkeyKey::AltLeft);

        assert_eq!(state.key_up(HotkeyKey::CtrlLeft).action, None);
    }

    #[test]
    fn escape_is_only_consumed_while_a_dismiss_target_is_active() {
        let mut state = KeyState::default();

        assert_eq!(state.escape_down(false), HotkeyOutcome::default());
        assert_eq!(
            state.escape_down(true),
            HotkeyOutcome {
                action: Some(HotkeyAction::DismissEscapeTarget),
                suppress: true,
            }
        );
        assert_eq!(
            state.escape_down(false),
            HotkeyOutcome {
                action: None,
                suppress: true,
            }
        );
        assert_eq!(
            state.escape_up(),
            HotkeyOutcome {
                action: None,
                suppress: true,
            }
        );
        assert_eq!(state.escape_up(), HotkeyOutcome::default());
    }
}
