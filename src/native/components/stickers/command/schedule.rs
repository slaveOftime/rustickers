//! Cron scheduling for command stickers.
//!
//! Expressions are Quartz style with 6 or 7 fields (`sec min hour day month weekday [year]`), so
//! the familiar 5 field crontab syntax is rejected. The UI offers `0 */1 * * * *` as the default.

use std::{str::FromStr, sync::Arc, sync::atomic::AtomicBool, sync::atomic::Ordering};

use chrono::{DateTime, Local};

use crate::model::content::{CommandContent, Scheduler};

/// How the next fire time is shown to the user.
pub const DISPLAY_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// The cron expression that arms this sticker, if it has one.
///
/// An all-whitespace expression counts as no schedule: the settings form leaves the input empty
/// when the user switches the schedule off.
pub fn cron_expr(content: &CommandContent) -> Option<&str> {
    match content.scheduler.as_ref()? {
        Scheduler::Cron(expr) => {
            let expr = expr.trim();
            (!expr.is_empty()).then_some(expr)
        }
    }
}

/// A schedule only ticks once the user has started the sticker at least once.
pub fn is_armed(content: &CommandContent) -> bool {
    content.started_at.is_some() && cron_expr(content).is_some()
}

pub fn parse(expr: &str) -> Result<cron::Schedule, String> {
    cron::Schedule::from_str(expr).map_err(|err| format!("Invalid cron expression: {err}"))
}

/// The first fire time strictly after `after`.
pub fn next_after(schedule: &cron::Schedule, after: DateTime<Local>) -> Option<DateTime<Local>> {
    schedule.after(&after).next()
}

pub fn format(at: DateTime<Local>) -> String {
    at.format(DISPLAY_FORMAT).to_string()
}

/// A cooperative cancellation flag shared with a running schedule loop.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(scheduler: Option<Scheduler>, started_at: Option<i64>) -> CommandContent {
        CommandContent {
            scheduler,
            started_at,
            ..Default::default()
        }
    }

    #[test]
    fn blank_expressions_do_not_count_as_a_schedule() {
        assert_eq!(cron_expr(&content(None, Some(1))), None);
        assert_eq!(
            cron_expr(&content(Some(Scheduler::Cron("  ".into())), Some(1))),
            None
        );
        assert_eq!(
            cron_expr(&content(
                Some(Scheduler::Cron(" 0 * * * * * ".into())),
                Some(1)
            )),
            Some("0 * * * * *")
        );
    }

    #[test]
    fn arming_needs_both_a_schedule_and_a_start() {
        let expr = || Some(Scheduler::Cron("0 * * * * *".into()));

        assert!(is_armed(&content(expr(), Some(1))));
        assert!(!is_armed(&content(expr(), None)));
        assert!(!is_armed(&content(None, Some(1))));
    }

    #[test]
    fn five_field_crontab_syntax_is_rejected() {
        assert!(parse("*/1 * * * *").is_err());
        assert!(parse("0 */1 * * * *").is_ok());
    }

    #[test]
    fn next_fire_time_is_strictly_in_the_future() {
        let schedule = parse("0 * * * * *").expect("valid");
        let now = Local::now();

        let next = next_after(&schedule, now).expect("cron has upcoming times");

        assert!(next > now);
    }

    #[test]
    fn cancelling_is_observable() {
        let token = CancelToken::new();
        let clone = token.clone();

        assert!(!clone.is_cancelled());
        token.cancel();
        assert!(clone.is_cancelled());
    }
}
