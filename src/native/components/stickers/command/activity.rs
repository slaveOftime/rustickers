//! Process-wide view of what every command sticker is doing right now.
//!
//! Two very different places write here: the sticker window while the user watches a command run,
//! and the headless [`super::background`] scheduler while the sticker is closed. The main window's
//! list reads it to draw its indicators, and the scheduler reads it to stay out of the way of an
//! open window.
//!
//! State lives in a process-wide registry rather than on an entity because it has to outlive the
//! sticker window: the whole point of a background schedule is that it keeps running after the
//! window is gone.

use std::{
    collections::HashMap,
    sync::{LazyLock, RwLock},
};

static REGISTRY: LazyLock<RwLock<Registry>> = LazyLock::new(Default::default);

#[derive(Default)]
struct Registry {
    /// Reference counted so an unexpected overlap can never leave a sticker stuck as "running".
    running: HashMap<i64, u32>,
    /// Stickers whose window is open, and which therefore drive their own schedule.
    window_owned: HashMap<i64, u32>,
    /// Formatted next fire time per background scheduled sticker.
    next_run: HashMap<i64, String>,
    /// Bumped on every change, so views can repaint without subscribing to anything.
    generation: u64,
}

impl Registry {
    fn increment(counts: &mut HashMap<i64, u32>, id: i64) {
        *counts.entry(id).or_default() += 1;
    }

    fn decrement(counts: &mut HashMap<i64, u32>, id: i64) {
        if let Some(count) = counts.get_mut(&id) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&id);
            }
        }
    }
}

fn mutate(change: impl FnOnce(&mut Registry)) {
    let Ok(mut registry) = REGISTRY.write() else {
        return;
    };
    change(&mut registry);
    registry.generation = registry.generation.wrapping_add(1);
}

fn read<T>(view: impl FnOnce(&Registry) -> T, fallback: T) -> T {
    match REGISTRY.read() {
        Ok(registry) => view(&registry),
        Err(_) => fallback,
    }
}

/// A counter that only changes when something a view might draw has changed.
///
/// Polling this is far cheaper than diffing the registry, and avoids threading yet another event
/// channel from the scheduler into the main window.
pub fn generation() -> u64 {
    read(|registry| registry.generation, 0)
}

pub fn is_running(id: i64) -> bool {
    read(|registry| registry.running.contains_key(&id), false)
}

/// The formatted time this sticker is next due to run in the background, if it is scheduled.
pub fn next_run(id: i64) -> Option<String> {
    read(|registry| registry.next_run.get(&id).cloned(), None)
}

pub fn is_window_owned(id: i64) -> bool {
    read(|registry| registry.window_owned.contains_key(&id), false)
}

pub fn set_next_run(id: i64, at: String) {
    let unchanged = read(
        |registry| {
            registry
                .next_run
                .get(&id)
                .is_some_and(|current| *current == at)
        },
        false,
    );
    if unchanged {
        return;
    }
    mutate(|registry| {
        registry.next_run.insert(id, at);
    });
}

pub fn clear_next_run(id: i64) {
    if !read(|registry| registry.next_run.contains_key(&id), false) {
        return;
    }
    mutate(|registry| {
        registry.next_run.remove(&id);
    });
}

/// Retain only the given ids, forgetting stickers that are no longer scheduled or were deleted.
pub fn retain_scheduled(ids: &[i64]) {
    let stale = read(
        |registry| {
            registry
                .next_run
                .keys()
                .copied()
                .filter(|id| !ids.contains(id))
                .collect::<Vec<_>>()
        },
        Vec::new(),
    );

    if stale.is_empty() {
        return;
    }

    mutate(|registry| {
        for id in stale {
            registry.next_run.remove(&id);
        }
    });
}

/// Mark a sticker as running until the returned guard is dropped.
pub fn begin_run(id: i64) -> RunGuard {
    mutate(|registry| Registry::increment(&mut registry.running, id));
    RunGuard(id)
}

/// Claim a sticker for an open window until the returned guard is dropped.
///
/// While claimed the background scheduler leaves the sticker alone, so a command can never be
/// started twice for the same tick.
pub fn claim_window(id: i64) -> WindowClaim {
    mutate(|registry| Registry::increment(&mut registry.window_owned, id));
    WindowClaim(id)
}

pub struct RunGuard(i64);

impl Drop for RunGuard {
    fn drop(&mut self) {
        let id = self.0;
        mutate(|registry| Registry::decrement(&mut registry.running, id));
    }
}

pub struct WindowClaim(i64);

impl Drop for WindowClaim {
    fn drop(&mut self) {
        let id = self.0;
        mutate(|registry| Registry::decrement(&mut registry.window_owned, id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry is process wide, so tests use disjoint ids. The two that exercise
    /// `retain_scheduled` also serialise, because purging is global by nature.
    static SCHEDULE_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_run_guard_marks_the_sticker_until_it_is_dropped() {
        let id = -9001;
        assert!(!is_running(id));

        let guard = begin_run(id);
        assert!(is_running(id));

        drop(guard);
        assert!(!is_running(id));
    }

    #[test]
    fn overlapping_runs_are_reference_counted() {
        let id = -9002;
        let first = begin_run(id);
        let second = begin_run(id);

        drop(first);
        assert!(is_running(id), "still running under the second guard");

        drop(second);
        assert!(!is_running(id));
    }

    #[test]
    fn a_window_claim_keeps_the_scheduler_away() {
        let id = -9003;
        assert!(!is_window_owned(id));

        let claim = claim_window(id);
        assert!(is_window_owned(id));

        drop(claim);
        assert!(!is_window_owned(id));
    }

    #[test]
    fn the_generation_only_moves_on_a_real_change() {
        let _guard = SCHEDULE_TESTS.lock();
        let id = -9004;
        set_next_run(id, "2030-01-01 00:00:00".to_string());

        let before = generation();
        set_next_run(id, "2030-01-01 00:00:00".to_string());
        assert_eq!(generation(), before, "an identical value is not a change");

        set_next_run(id, "2030-01-01 00:01:00".to_string());
        assert_ne!(generation(), before);

        clear_next_run(id);
        assert_eq!(next_run(id), None);
    }

    #[test]
    fn retaining_forgets_stickers_that_are_no_longer_scheduled() {
        let _guard = SCHEDULE_TESTS.lock();
        let kept = -9005;
        let dropped = -9006;
        set_next_run(kept, "2030-01-01 00:00:00".to_string());
        set_next_run(dropped, "2030-01-01 00:00:00".to_string());

        retain_scheduled(&[kept]);

        assert!(next_run(kept).is_some());
        assert!(next_run(dropped).is_none());
    }
}
