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

    let preview_combo_down = Arc::new(AtomicBool::new(false));
    let preview_combo_down_for_tap = preview_combo_down.clone();
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

            let preview_combo = primary && alt;
            let preview_combo_was_down =
                preview_combo_down_for_tap.swap(preview_combo, Ordering::Relaxed);

            match event_type {
                CGEventType::FlagsChanged if preview_combo && !preview_combo_was_down => {
                    tracing::debug!("Hotkey triggered: toggle file preview");
                    let _ = ipc_events_tx.send(IpcEvent::ToggleFilePreview);
                }
                CGEventType::KeyDown
                    if primary
                        && alt
                        && event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) == KEY_R
                        && event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) == 0 =>
                {
                    tracing::debug!("Hotkey triggered: show");
                    let _ = ipc_events_tx.send(IpcEvent::Show);
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
                        // Debounce key-repeat while held.
                        if !state.alt {
                            state.alt = true;
                            if primary_modifier_down(*state) {
                                tracing::debug!(
                                    alt = state.alt,
                                    ctrl = state.ctrl,
                                    meta = state.meta,
                                    "Hotkey triggered: toggle file preview"
                                );
                                let _ = ipc_events_tx.send(IpcEvent::ToggleFilePreview);
                            }
                        }
                    }
                    Key::ControlLeft | Key::ControlRight => {
                        state.ctrl = true;
                        if state.alt && primary_modifier_down(*state) {
                            tracing::debug!(
                                alt = state.alt,
                                ctrl = state.ctrl,
                                meta = state.meta,
                                "Hotkey triggered: toggle file preview"
                            );
                            let _ = ipc_events_tx.send(IpcEvent::ToggleFilePreview);
                        }
                    }
                    Key::ShiftLeft | Key::ShiftRight => state.shift = true,
                    Key::MetaLeft | Key::MetaRight => state.meta = true,
                    Key::KeyR => {
                        // Debounce key-repeat while held.
                        if !state.r_down {
                            state.r_down = true;
                            if state.alt && primary_modifier_down(*state) {
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
                    _ => {}
                }
            }
            EventType::KeyRelease(key) => match key {
                Key::Alt => state.alt = false,
                Key::ControlLeft | Key::ControlRight => state.ctrl = false,
                Key::ShiftLeft | Key::ShiftRight => state.shift = false,
                Key::MetaLeft | Key::MetaRight => state.meta = false,
                Key::KeyR => state.r_down = false,
                _ => {}
            },
            _ => {}
        }
    };

    listen(callback).map_err(|err| anyhow::anyhow!("rdev listen failed: {err:?}"))
}
