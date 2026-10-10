//! OS-independent matching, repeat suppression, and modifier-only gestures.
#[cfg(any(target_os = "macos", test))]
use super::bindings::{ALT, CTRL, SHIFT};
use super::{HotkeyAction, bindings::Snapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Key {
    Modifier(u8, u8),
    Code(u16),
    Other,
}
#[derive(Default, Debug, PartialEq, Eq)]
pub(super) struct Outcome {
    pub(super) action: Option<HotkeyAction>,
    pub(super) suppress: bool,
}
pub(super) struct Matcher {
    sides: [u8; 4],
    down: [bool; 256],
    suppressed: [bool; 256],
    gesture: bool,
    consumed: bool,
    revision: u64,
    waiting_for_release: bool,
    escape_suppressed: bool,
}
impl Default for Matcher {
    fn default() -> Self {
        Self {
            sides: [0; 4],
            down: [false; 256],
            suppressed: [false; 256],
            gesture: false,
            consumed: false,
            revision: 0,
            waiting_for_release: false,
            escape_suppressed: false,
        }
    }
}
impl Matcher {
    fn modifiers(&self) -> u8 {
        self.sides.iter().enumerate().fold(0, |mask, (ix, sides)| {
            mask | if *sides != 0 { 1 << ix } else { 0 }
        })
    }
    fn active(&self) -> bool {
        self.modifiers() != 0 || self.down.iter().any(|d| *d)
    }
    pub(super) fn event(&mut self, key: Key, pressed: bool, snapshot: Snapshot) -> Outcome {
        if self.revision != snapshot.revision {
            self.revision = snapshot.revision;
            self.waiting_for_release = self.active();
            self.gesture = false;
            self.consumed = true;
        }
        let previous_modifiers = self.modifiers();
        let mut outcome = Outcome::default();
        match key {
            Key::Modifier(modifier, side) => {
                let ix = modifier.trailing_zeros() as usize;
                if pressed {
                    self.sides[ix] |= side;
                } else {
                    self.sides[ix] &= !side;
                }
            }
            Key::Code(code) if code < 256 => {
                let ix = code as usize;
                if pressed {
                    let repeat = self.down[ix];
                    self.down[ix] = true;
                    self.consumed = true;
                    outcome.suppress = self.suppressed[ix];
                    if !repeat
                        && !self.waiting_for_release
                        && let Some(slot) = snapshot.chords.iter().position(|chord| {
                            chord.key == Some(code) && chord.modifiers == self.modifiers()
                        })
                    {
                        self.suppressed[ix] = true;
                        outcome.suppress = true;
                        outcome.action = Some(HotkeyAction::from_slot(slot));
                    }
                } else {
                    self.down[ix] = false;
                    outcome.suppress = self.suppressed[ix];
                    self.suppressed[ix] = false;
                }
            }
            _ => {
                if pressed {
                    self.consumed = true;
                }
            }
        }
        let preview = snapshot.chords[2];
        let modifiers = self.modifiers();
        if !self.waiting_for_release && preview.key.is_none() {
            if pressed
                && !self.gesture
                && matches!(key, Key::Modifier(_, _))
                && modifiers == preview.modifiers
                && previous_modifiers != modifiers
            {
                self.gesture = true;
                self.consumed = self.down.iter().any(|d| *d);
            } else if self.gesture && modifiers & !preview.modifiers != 0 {
                self.consumed = true;
            }
            if self.gesture && modifiers == 0 {
                if !self.consumed {
                    outcome.action = Some(HotkeyAction::ToggleFilePreview);
                }
                self.gesture = false;
            }
        }
        // Drop any residual non-modifier state once the user has lifted every
        // chord modifier. The OS can drop a key-up event during a foreground
        // transition (e.g. UAC prompt, lock screen, RDP focus swap), which
        // would otherwise leave `down` (and `suppressed`) pinned for a virtual
        // key that the user already released. The follow-up press would then
        // register as a repeat (`repeat = self.down[ix] == true`) and the
        // chord would never fire again until the app restarts. Releasing the
        // last modifier is a clean baseline: any next chord must be entered
        // from scratch, so forgetting the phantom "still held" entry is safe.
        //
        // Only `down` is reset here. `suppressed` is left intact so that an
        // unmatched release (e.g. the chord key released *after* every
        // modifier) still suppresses the trailing release to the foreground
        // app, matching the documented chord-key suppression behaviour.
        // The next chord match clears it naturally.
        if previous_modifiers != 0 && modifiers == 0 {
            self.down = [false; 256];
        }
        if self.waiting_for_release && !self.active() {
            self.waiting_for_release = false;
            self.gesture = false;
        }
        outcome
    }
    #[cfg(any(target_os = "macos", test))]
    pub(super) fn flags(&mut self, modifiers: u8, snapshot: Snapshot) -> Outcome {
        // macOS FlagsChanged supplies aggregate state; preserve all transitions in one event.
        let old = self.modifiers();
        if self.revision != snapshot.revision {
            self.revision = snapshot.revision;
            self.waiting_for_release = self.active() || modifiers != 0;
            self.gesture = false;
            self.consumed = true;
        }
        let mut result = Outcome::default();
        // Add modifiers before removing others: a chord replacement must not
        // transiently look like all modifiers were released.
        for modifier in [CTRL, ALT, SHIFT]
            .into_iter()
            .filter(|m| modifiers & m != 0)
            .chain(
                [CTRL, ALT, SHIFT]
                    .into_iter()
                    .filter(|m| modifiers & m == 0),
            )
        {
            if (old & modifier != 0) != (modifiers & modifier != 0) {
                let next = self.event(
                    Key::Modifier(modifier, 1),
                    modifiers & modifier != 0,
                    snapshot,
                );
                if next.action.is_some() {
                    result = next;
                }
            }
        }
        result
    }
    pub(super) fn escape(&mut self, pressed: bool, target: bool) -> Outcome {
        if pressed {
            self.consumed = true;
            if self.escape_suppressed {
                return Outcome {
                    action: None,
                    suppress: true,
                };
            }
            if target {
                self.escape_suppressed = true;
                return Outcome {
                    action: Some(HotkeyAction::DismissEscapeTarget),
                    suppress: true,
                };
            }
            self.consumed = true;
            Outcome::default()
        } else {
            let suppress = std::mem::take(&mut self.escape_suppressed);
            Outcome {
                action: None,
                suppress,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(values: [&str; 4]) -> Snapshot {
        Snapshot {
            chords: super::super::bindings::validate(&values.map(str::to_string)).unwrap(),
            revision: 0,
        }
    }
    fn defaults() -> Snapshot {
        config(["Ctrl+Shift+R", "Ctrl+Shift+V", "Ctrl+Alt", "Ctrl+Shift+Space"])
    }
    #[test]
    fn keyed_shortcuts_fire_once_and_suppress_repeat_and_release() {
        for (code, slot, second) in [
            (82, 0, SHIFT),
            (86, 1, SHIFT),
            (32, 3, SHIFT),
        ] {
            let mut m = Matcher::default();
            let s = defaults();
            m.event(Key::Modifier(CTRL, 1), true, s);
            if second != 0 {
                m.event(Key::Modifier(second, 1), true, s);
            }
            assert_eq!(
                m.event(Key::Code(code), true, s),
                Outcome {
                    action: Some(HotkeyAction::from_slot(slot)),
                    suppress: true
                }
            );
            assert_eq!(
                m.event(Key::Code(code), true, s),
                Outcome {
                    action: None,
                    suppress: true
                }
            );
            m.event(Key::Modifier(CTRL, 1), false, s);
            assert!(m.event(Key::Code(code), false, s).suppress);
            if second != 0 {
                assert_eq!(m.event(Key::Modifier(second, 1), false, s).action, None);
            }
        }
    }
    #[test]
    fn bare_preview_waits_for_both_modifiers_and_never_fires_after_typing() {
        for typed in [false, true] {
            let mut m = Matcher::default();
            let s = defaults();
            m.event(Key::Modifier(CTRL, 1), true, s);
            m.event(Key::Modifier(ALT, 1), true, s);
            if typed {
                m.event(Key::Other, true, s);
            }
            assert!(m.event(Key::Modifier(ALT, 1), false, s).action.is_none());
            assert_eq!(
                m.event(Key::Modifier(CTRL, 1), false, s).action,
                if typed {
                    None
                } else {
                    Some(HotkeyAction::ToggleFilePreview)
                }
            );
        }
    }
    #[test]
    fn side_modifiers_and_exact_modifier_matching() {
        // Used an explicit snapshot rather than `defaults()`: this test
        // verifies that Ctrl+Shift is *not* collapsed to Ctrl when matching
        // slot 3, which only holds when slot 3 is `Ctrl+Space` (plain).
        let s = config(["Ctrl+Alt+R", "Ctrl+Alt+V", "Ctrl+Alt", "Ctrl+Space"]);
        let mut m = Matcher::default();
        m.event(Key::Modifier(CTRL, 1), true, s);
        m.event(Key::Modifier(CTRL, 2), true, s);
        m.event(Key::Modifier(CTRL, 1), false, s);
        assert_eq!(
            m.event(Key::Code(32), true, s).action,
            Some(HotkeyAction::TriggerSelectionToCommand)
        );
        let mut m = Matcher::default();
        m.event(Key::Modifier(CTRL, 1), true, s);
        m.event(Key::Modifier(SHIFT, 1), true, s);
        assert!(m.event(Key::Code(32), true, s).action.is_none());
    }
    #[test]
    fn a_consumed_preview_cannot_rearm_until_all_modifiers_are_released() {
        let mut m = Matcher::default();
        let s = defaults();
        m.event(Key::Modifier(CTRL, 1), true, s);
        m.event(Key::Modifier(ALT, 1), true, s);
        m.event(Key::Code(65), true, s);
        m.event(Key::Code(65), false, s);
        m.event(Key::Modifier(ALT, 1), false, s);
        m.event(Key::Modifier(ALT, 1), true, s);
        m.event(Key::Modifier(ALT, 1), false, s);
        assert!(m.event(Key::Modifier(CTRL, 1), false, s).action.is_none());
    }
    #[test]
    fn reload_while_held_preserves_suppression_and_prevents_phantom_preview() {
        let mut m = Matcher::default();
        let s = defaults();
        m.event(Key::Modifier(CTRL, 1), true, s);
        m.event(Key::Modifier(SHIFT, 1), true, s);
        m.event(Key::Code(86), true, s);
        let mut next = config(["Alt+F2", "Ctrl+Q", "Alt+Shift", "Ctrl+Space"]);
        next.revision = 1;
        assert!(m.event(Key::Code(86), false, next).suppress);
        m.event(Key::Modifier(CTRL, 1), false, next);
        assert!(m.event(Key::Modifier(SHIFT, 1), false, next).action.is_none());
        m.event(Key::Modifier(ALT, 1), true, next);
        assert_eq!(
            m.event(Key::Code(113), true, next).action,
            Some(HotkeyAction::Show)
        );
    }
    #[test]
    fn chord_recovers_after_missed_keyup_when_modifiers_release() {
        // The OS can drop a non-modifier key-up event during a foreground
        // transition (UAC, lock screen, RDP focus swap). Without state
        // recovery, the matcher would treat the next press of the same chord
        // as a repeat and the action would never fire again until the app
        // restarts. Lifting the last modifier must reset the phantom
        // "still held" entries so the chord can fire again.
        let s = defaults();

        // First attempt: chord fires, but the V release is lost mid-flight.
        let mut m = Matcher::default();
        m.event(Key::Modifier(CTRL, 1), true, s);
        m.event(Key::Modifier(SHIFT, 1), true, s);
        assert_eq!(
            m.event(Key::Code(86), true, s).action,
            Some(HotkeyAction::ToggleClipboardChooser)
        );
        // V-up never arrives; user just releases the modifiers.
        m.event(Key::Modifier(SHIFT, 1), false, s);
        m.event(Key::Modifier(CTRL, 1), false, s);

        // Second attempt: chord must fire again, not be dropped as a repeat.
        m.event(Key::Modifier(CTRL, 1), true, s);
        m.event(Key::Modifier(SHIFT, 1), true, s);
        assert_eq!(
            m.event(Key::Code(86), true, s).action,
            Some(HotkeyAction::ToggleClipboardChooser)
        );

        // After the recovered match, the trailing V release must still be
        // suppressed so the foreground app doesn't see a stray 'v' keystroke.
        assert!(m.event(Key::Code(86), false, s).suppress);

        // Once the user releases the chord modifiers too, plain V presses
        // must no longer be suppressed by any stray state.
        m.event(Key::Modifier(SHIFT, 1), false, s);
        m.event(Key::Modifier(CTRL, 1), false, s);
        assert!(!m.event(Key::Code(86), true, s).suppress);
        assert!(!m.event(Key::Code(86), false, s).suppress);
    }
    #[test]
    fn custom_modifier_preview_and_keyed_preview_work() {
        let mut m = Matcher::default();
        let s = config(["Ctrl+R", "Ctrl+V", "Alt+Shift", "Ctrl+Space"]);
        m.event(Key::Modifier(ALT, 1), true, s);
        m.event(Key::Modifier(SHIFT, 1), true, s);
        m.event(Key::Modifier(ALT, 1), false, s);
        assert_eq!(
            m.event(Key::Modifier(SHIFT, 1), false, s).action,
            Some(HotkeyAction::ToggleFilePreview)
        );
        let mut m = Matcher::default();
        let s = config(["Ctrl+R", "Ctrl+V", "Alt+F3", "Ctrl+Space"]);
        m.event(Key::Modifier(ALT, 1), true, s);
        assert_eq!(
            m.event(Key::Code(114), true, s).action,
            Some(HotkeyAction::ToggleFilePreview)
        );
    }
    #[test]
    fn aggregate_flags_preserve_modifier_gesture_semantics() {
        let mut m = Matcher::default();
        let s = defaults();
        m.flags(CTRL | ALT, s);
        assert!(m.flags(SHIFT, s).action.is_none());
        assert!(m.flags(0, s).action.is_none());
        m.flags(CTRL | ALT, s);
        assert_eq!(m.flags(0, s).action, Some(HotkeyAction::ToggleFilePreview));
    }
    #[test]
    fn escape_remains_scoped_and_repeat_suppressed() {
        let mut m = Matcher::default();
        assert_eq!(m.escape(true, false), Outcome::default());
        assert_eq!(
            m.escape(true, true).action,
            Some(HotkeyAction::DismissEscapeTarget)
        );
        assert!(m.escape(true, false).suppress);
        assert!(m.escape(false, false).suppress);
        assert!(!m.escape(false, false).suppress);
    }
}
