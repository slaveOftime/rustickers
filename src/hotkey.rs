use std::sync::{Arc, Mutex, mpsc::Sender};

use crate::ipc::IpcEvent;

#[derive(Default, Debug, Clone, Copy)]
struct KeyState {
    ctrl: bool,
    shift: bool,
    meta: bool,
    s_down: bool,
}

fn primary_modifier_down(state: KeyState) -> bool {
    if cfg!(target_os = "macos") {
        // On macOS, users commonly expect Command; allow Control too.
        state.meta || state.ctrl
    } else {
        state.ctrl
    }
}

pub fn start_global_hotkey_listener(ipc_events_tx: Sender<IpcEvent>) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("global-hotkey-listener".to_string())
        .spawn(move || {
            if let Err(err) = start_listen(ipc_events_tx) {
                eprintln!("Global hotkey listener stopped: {err:#}");
            }
        })?;

    Ok(())
}

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
                    Key::ControlLeft | Key::ControlRight => state.ctrl = true,
                    Key::ShiftLeft | Key::ShiftRight => state.shift = true,
                    Key::MetaLeft | Key::MetaRight => state.meta = true,
                    Key::KeyS => {
                        // Debounce key-repeat while held.
                        if !state.s_down {
                            state.s_down = true;
                            if state.shift && primary_modifier_down(*state) {
                                let _ = ipc_events_tx.send(IpcEvent::Show);
                            }
                        }
                    }
                    _ => {}
                }
            }
            EventType::KeyRelease(key) => match key {
                Key::ControlLeft | Key::ControlRight => state.ctrl = false,
                Key::ShiftLeft | Key::ShiftRight => state.shift = false,
                Key::MetaLeft | Key::MetaRight => state.meta = false,
                Key::KeyS => state.s_down = false,
                _ => {}
            },
            _ => {}
        }
    };

    listen(callback).map_err(|err| anyhow::anyhow!("rdev listen failed: {err:?}"))
}
