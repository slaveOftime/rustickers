//! Keeps armed cron command stickers ticking while their windows are closed.
//!
//! A sticker window owns its own schedule while it is open, because it streams output into the
//! view. The moment that window goes away the schedule used to die with it. This supervisor picks
//! those stickers up instead: it polls the store for anything armed with a cron expression, fires
//! the ones that are due, and writes the output straight back into the sticker's content so it is
//! waiting the next time the sticker is opened.
//!
//! Coordination with open windows goes through [`super::activity`]: a window claims its sticker for
//! as long as it lives, and a claimed sticker is skipped here. Its next fire time is still tracked
//! so the list view can show it.

use std::{collections::HashMap, sync::mpsc::TryRecvError, time::Duration};

use chrono::{DateTime, Local};
use gpui::{App, AsyncApp};

use crate::{
    model::content::CommandContent,
    native::components::stickers::command::{
        activity,
        runner::{self, CmdEvent, RunSpec},
        schedule,
    },
    storage::ArcStickerStore,
};

/// How often due jobs are checked. Cron resolution is one second, so this keeps drift under a tick
/// without waking up constantly.
const TICK: Duration = Duration::from_millis(500);

/// How often the job list is reloaded from the store, so schedules edited in a sticker window (or
/// through the CLI) are picked up without a restart.
const REFRESH_INTERVAL_SECS: i64 = 3;

/// Parked deadline for a schedule that has no upcoming times left, so it stops being due without
/// having to mutate the map while iterating it.
const NEVER_AGAIN_DAYS: i64 = 3650;

/// How often a running job's output channel is drained.
const OUTPUT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Start supervising background schedules. Call once, at startup.
pub fn start(cx: &mut App, store: ArcStickerStore) {
    cx.spawn(async move |cx| supervise(store, cx).await)
        .detach();
}

struct Job {
    /// Kept so an edited expression is detected and the next fire time recomputed.
    expr: String,
    schedule: cron::Schedule,
    next_at: DateTime<Local>,
    content: CommandContent,
}

async fn supervise(store: ArcStickerStore, cx: &mut AsyncApp) {
    tracing::info!("Background command scheduler started");

    let mut jobs: HashMap<i64, Job> = HashMap::new();
    let mut next_refresh = Local::now();

    loop {
        let now = Local::now();

        if now >= next_refresh {
            next_refresh = now + chrono::Duration::seconds(REFRESH_INTERVAL_SECS);
            refresh(&store, &mut jobs, now).await;
        }

        for id in due_jobs(&mut jobs, now) {
            let Some(job) = jobs.get(&id) else { continue };
            let content = job.content.clone();
            let store = store.clone();
            cx.spawn(async move |cx| run_once(id, content, store, cx).await)
                .detach();
        }

        cx.background_executor().timer(TICK).await;
    }
}

/// Reload the armed stickers, preserving the countdown of jobs whose expression has not changed.
async fn refresh(store: &ArcStickerStore, jobs: &mut HashMap<i64, Job>, now: DateTime<Local>) {
    let rows = match store.get_scheduled_command_stickers().await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(error = ?err, "Failed to load scheduled command stickers");
            return;
        }
    };

    let mut seen = Vec::with_capacity(rows.len());

    for row in rows {
        let content = match serde_json::from_str::<CommandContent>(&row.content) {
            Ok(content) => content,
            Err(err) => {
                tracing::warn!(id = row.id, error = ?err, "Skipping command sticker with unreadable content");
                continue;
            }
        };

        // The SQL filter is a cheap pre-selection; `is_armed` is the authority.
        let Some(expr) = schedule::cron_expr(&content).filter(|_| schedule::is_armed(&content))
        else {
            continue;
        };

        let parsed = match schedule::parse(expr) {
            Ok(parsed) => parsed,
            Err(err) => {
                tracing::warn!(id = row.id, expr, error = %err, "Ignoring invalid cron expression");
                continue;
            }
        };

        seen.push(row.id);

        match jobs.get_mut(&row.id) {
            // Same schedule: keep counting down, but pick up any other edits to the command.
            Some(job) if job.expr == expr => {
                job.content = content;
                continue;
            }
            _ => {}
        }

        let Some(next_at) = schedule::next_after(&parsed, now) else {
            tracing::warn!(id = row.id, expr, "Cron expression has no upcoming times");
            continue;
        };

        tracing::debug!(id = row.id, expr, next_at = %schedule::format(next_at), "Scheduling command sticker in the background");
        activity::set_next_run(row.id, schedule::format(next_at));
        jobs.insert(
            row.id,
            Job {
                expr: expr.to_string(),
                schedule: parsed,
                next_at,
                content,
            },
        );
    }

    jobs.retain(|id, _| seen.contains(id));
    activity::retain_scheduled(&seen);
}

/// Advance every job that is due and return the ones that should actually run now.
///
/// The countdown advances even for jobs that are skipped, otherwise a sticker whose window stayed
/// open for an hour would fire a backlog of runs the moment it closed.
fn due_jobs(jobs: &mut HashMap<i64, Job>, now: DateTime<Local>) -> Vec<i64> {
    let mut ready = Vec::new();

    for (id, job) in jobs.iter_mut() {
        if now < job.next_at {
            continue;
        }

        match schedule::next_after(&job.schedule, now) {
            Some(next_at) => {
                job.next_at = next_at;
                activity::set_next_run(*id, schedule::format(next_at));
            }
            None => {
                // Nothing further to run: park the deadline so it never becomes due again.
                job.next_at = now + chrono::Duration::days(NEVER_AGAIN_DAYS);
                activity::clear_next_run(*id);
            }
        }

        if activity::is_window_owned(*id) {
            tracing::debug!(
                id,
                "Skipping background run, the sticker window owns the schedule"
            );
            continue;
        }

        if activity::is_running(*id) {
            tracing::debug!(
                id,
                "Skipping background run, the previous one is still going"
            );
            continue;
        }

        ready.push(*id);
    }

    ready
}

/// Execute one scheduled run and persist whatever it produced.
async fn run_once(id: i64, content: CommandContent, store: ArcStickerStore, cx: &mut AsyncApp) {
    let _guard = activity::begin_run(id);

    let output = match RunSpec::resolve(&content, None) {
        Ok(spec) => collect(&spec, cx).await,
        Err(err) => Err(err),
    };

    let (success, text) = match output {
        Ok(result) => result,
        Err(err) => {
            tracing::warn!(id, error = %err, "Background command failed to start");
            // Recorded as the result so the failure is visible when the sticker is next opened,
            // rather than only living in the log.
            (false, err)
        }
    };

    tracing::debug!(id, success, "Background command run finished");

    if let Err(err) = persist(&store, id, text).await {
        tracing::warn!(id, error = ?err, "Failed to save background command result");
    }
}

/// Drain the child's output without blocking the foreground thread.
async fn collect(spec: &RunSpec, cx: &mut AsyncApp) -> Result<(bool, String), String> {
    let child = spec.spawn()?;
    // `pump` does the blocking reads on its own threads, so this loop only ever polls.
    let (_child, rx) = runner::pump(child);

    let mut text = String::new();
    let mut success = false;

    loop {
        match rx.try_recv() {
            Ok(CmdEvent::Output(line) | CmdEvent::Error(line)) => {
                text.push_str(&line);
                text.push('\n');
            }
            Ok(CmdEvent::Done { success: ok }) => {
                success = ok;
                break;
            }
            Err(TryRecvError::Empty) => {
                cx.background_executor().timer(OUTPUT_POLL_INTERVAL).await;
            }
            Err(TryRecvError::Disconnected) => break,
        }
    }

    Ok((success, text))
}

/// Write the output into the sticker, leaving every other setting untouched.
///
/// The content is re-read rather than reusing the copy the job was built from, so a run that
/// started before the user edited the sticker cannot roll those edits back.
async fn persist(store: &ArcStickerStore, id: i64, output: String) -> anyhow::Result<()> {
    let detail = store.get_sticker(id).await?;
    let mut content = serde_json::from_str::<CommandContent>(&detail.content)?;
    content.result.set(Some(output));
    store
        .update_sticker_content(id, serde_json::to_string(&content)?)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(expr: &str, next_at: DateTime<Local>) -> Job {
        Job {
            expr: expr.to_string(),
            schedule: schedule::parse(expr).expect("valid"),
            next_at,
            content: CommandContent::default(),
        }
    }

    #[test]
    fn only_jobs_past_their_deadline_are_due() {
        let now = Local::now();
        let mut jobs = HashMap::from([
            (
                -8001,
                job("0 * * * * *", now - chrono::Duration::seconds(1)),
            ),
            (-8002, job("0 * * * * *", now + chrono::Duration::hours(1))),
        ]);

        assert_eq!(due_jobs(&mut jobs, now), [-8001]);
    }

    #[test]
    fn a_due_job_is_rearmed_into_the_future() {
        let now = Local::now();
        let mut jobs =
            HashMap::from([(-8003, job("0 * * * * *", now - chrono::Duration::hours(2)))]);

        due_jobs(&mut jobs, now);

        assert!(
            jobs[&-8003].next_at > now,
            "a missed deadline must not fire repeatedly"
        );
        assert!(due_jobs(&mut jobs, now).is_empty());
    }

    #[test]
    fn a_sticker_owned_by_its_window_is_rearmed_but_not_run() {
        let now = Local::now();
        let id = -8004;
        let mut jobs =
            HashMap::from([(id, job("0 * * * * *", now - chrono::Duration::seconds(1)))]);

        let _claim = activity::claim_window(id);

        assert!(due_jobs(&mut jobs, now).is_empty());
        assert!(jobs[&id].next_at > now);
    }
}
