#[derive(Debug, Clone, Copy)]
pub struct TransientTopmost {
    armed: bool,
    presented: bool,
    ignore_initial_inactive: bool,
}

impl TransientTopmost {
    pub fn new(armed: bool, initially_visible: bool) -> Self {
        Self {
            armed,
            presented: armed && initially_visible,
            ignore_initial_inactive: armed && initially_visible,
        }
    }

    pub fn update_activation(&mut self, active: bool) -> bool {
        if !self.armed {
            return false;
        }
        if active {
            self.presented = true;
            self.ignore_initial_inactive = false;
            return false;
        }
        if self.ignore_initial_inactive {
            self.ignore_initial_inactive = false;
            return false;
        }
        if self.presented {
            self.armed = false;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_inactive_callback_does_not_demote() {
        let mut state = TransientTopmost::new(true, true);

        assert!(!state.update_activation(false));
        assert!(state.update_activation(false));
    }

    #[test]
    fn demotes_only_once_after_first_activation() {
        let mut state = TransientTopmost::new(true, true);

        state.update_activation(true);
        assert!(state.update_activation(false));
        assert!(!state.update_activation(true));
        assert!(!state.update_activation(false));
    }

    #[test]
    fn permanent_topmost_state_never_demotes() {
        let mut state = TransientTopmost::new(false, true);

        assert!(!state.update_activation(true));
        assert!(!state.update_activation(false));
    }

    #[test]
    fn hidden_window_waits_until_it_has_been_activated() {
        let mut state = TransientTopmost::new(true, false);

        assert!(!state.update_activation(false));
        assert!(!state.update_activation(true));
        assert!(state.update_activation(false));
    }
}
