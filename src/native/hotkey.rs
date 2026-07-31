use std::sync::mpsc::Sender;

#[cfg(not(target_os = "macos"))]
use std::sync::{Arc, Mutex};

use crate::ipc::IpcEvent;

#[cfg(not(target_os = "macos"))]
#[derive(Default, Debug, Clone, Copy)]
struct KeyState {
    ctrl: bool,
    shift: bool,
    alt: bool,
    meta: bool,
    r_down: bool,
    space_down: bool,
    modifier_combo_active: bool,
    modifier_combo_consumed: bool,
}

#[cfg(not(target_os = "macos"))]
fn primary_modifier_down(state: KeyState) -> bool {
    state.ctrl
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

    // Use a listen-only Quartz event tap instead of rdev on macOS. rdev translates
    // key codes through Text Input Services from this worker thread, which violates
    // the main-queue requirement on recent macOS releases and crashes the process.
    // Quartz key codes and modifier flags need no keyboard-layout translation.
    const KEY_R: i64 = 0x0f;
    const KEY_SPACE: i64 = 0x31;

    let modifier_combo_down = Arc::new(AtomicBool::new(false));
    let modifier_combo_down_for_tap = modifier_combo_down.clone();
    let modifier_combo_consumed = Arc::new(AtomicBool::new(false));
    let modifier_combo_consumed_for_tap = modifier_combo_consumed.clone();
    let tap = CGEventTap::new(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::FlagsChanged, CGEventType::KeyDown],
        move |_, event_type, event| {
            let flags = event.get_flags();
            let primary = flags.intersects(CGEventFlags::CGEventFlagCommand)
                || flags.intersects(CGEventFlags::CGEventFlagControl);
            let alt = flags.intersects(CGEventFlags::CGEventFlagAlternate);

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
                        tracing::debug!("Hotkey triggered: toggle file preview");
                        let _ = ipc_events_tx.send(IpcEvent::ToggleFilePreview);
                    }
                }
                CGEventType::KeyDown
                    if primary
                        && alt
                        && event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) == KEY_R
                        && event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) == 0 =>
                {
                    modifier_combo_consumed_for_tap.store(true, Ordering::Relaxed);
                    tracing::debug!("Hotkey triggered: show");
                    let _ = ipc_events_tx.send(IpcEvent::Show);
                }
                CGEventType::KeyDown
                    if primary
                        && !alt
                        && event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) == KEY_SPACE
                        && event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) == 0 =>
                {
                    tracing::debug!("Hotkey triggered: selection to command");
                    let _ = ipc_events_tx.send(IpcEvent::TriggerSelectionToCommand);
                }
                _ => {}
            }

            None
        },
    )
    .map_err(|_| {
        anyhow::anyhow!(
            "failed to create macOS event tap; enable Input Monitoring for Rustickers in System Settings > Privacy & Security"
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

#[cfg(not(target_os = "macos"))]
fn start_listen(ipc_events_tx: Sender<IpcEvent>) -> anyhow::Result<()> {
    use rdev::{Event, EventType, Key, listen};

    let state = Arc::new(Mutex::new(KeyState::default()));
    let state_for_cb = state.clone();

    let callback = move |event: Event| {
        let mut state = match state_for_cb.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        match event.event_type {
            EventType::KeyPress(key) => {
                match key {
                    Key::Alt => {
                        if !state.alt {
                            state.alt = true;
                            if primary_modifier_down(*state) && !state.modifier_combo_active {
                                state.modifier_combo_active = true;
                                state.modifier_combo_consumed = false;
                            }
                        }
                    }
                    Key::ControlLeft | Key::ControlRight => {
                        state.ctrl = true;
                        if state.alt && !state.modifier_combo_active {
                            state.modifier_combo_active = true;
                            state.modifier_combo_consumed = false;
                        }
                    }
                    Key::ShiftLeft | Key::ShiftRight => state.shift = true,
                    Key::MetaLeft | Key::MetaRight => state.meta = true,
                    Key::KeyR => {
                        // Debounce key-repeat while held.
                        if !state.r_down {
                            state.r_down = true;
                            if state.alt && primary_modifier_down(*state) {
                                state.modifier_combo_consumed = true;
                                tracing::debug!(
                                    alt = state.alt,
                                    ctrl = state.ctrl,
                                    meta = state.meta,
                                    "Hotkey triggered: show"
                                );
                                let _ = ipc_events_tx.send(IpcEvent::Show);
                            }
                        }
                    }
                    Key::Space => {
                        if !state.space_down {
                            state.space_down = true;
                            if !state.alt && primary_modifier_down(*state) {
                                tracing::debug!("Hotkey triggered: selection to command");
                                let _ = ipc_events_tx.send(IpcEvent::TriggerSelectionToCommand);
                            }
                        }
                    }
                    _ => {}
                }
            }
            EventType::KeyRelease(key) => match key {
                Key::Alt | Key::ControlLeft | Key::ControlRight => {
                    match key {
                        Key::Alt => state.alt = false,
                        Key::ControlLeft | Key::ControlRight => state.ctrl = false,
                        _ => {}
                    }
                    if state.modifier_combo_active && !state.alt && !state.ctrl {
                        if !state.modifier_combo_consumed {
                            tracing::debug!("Hotkey triggered: toggle file preview");
                            let _ = ipc_events_tx.send(IpcEvent::ToggleFilePreview);
                        }
                        state.modifier_combo_active = false;
                        state.modifier_combo_consumed = false;
                    }
                }
                Key::ShiftLeft | Key::ShiftRight => state.shift = false,
                Key::MetaLeft | Key::MetaRight => state.meta = false,
                Key::KeyR => state.r_down = false,
                Key::Space => state.space_down = false,
                _ => {}
            },
            _ => {}
        }
    };

    listen(callback).map_err(|err| anyhow::anyhow!("rdev listen failed: {err:?}"))
}
