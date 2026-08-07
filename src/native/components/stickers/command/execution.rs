//! Starting, streaming and stopping a command from its sticker window.
//!
//! The window drives its own schedule while it is open so output can be streamed live; once it
//! closes, [`super::background`] takes over from the same stored cron expression.

use std::{sync::mpsc::TryRecvError, time::Duration};

use gpui::{Context, Rgba, Window, prelude::*};

use crate::{
    model::content::{CommandResult, Scheduler},
    native::components::webview::SimpleWebView,
};

use super::{
    CommandSticker, CommandStickerWindowRequest, activity, output,
    runner::{self, CmdEvent, RunSpec},
    schedule,
};

/// The schedule wait is served in chunks instead of one long timer, because a timer cannot be
/// interrupted and the stop button has to take effect promptly.
const MAX_SLEEP_CHUNK: Duration = Duration::from_millis(250);

/// How long to wait before draining the output channel for the first time.
const OUTPUT_WARMUP: Duration = Duration::from_millis(100);

/// How often the output channel is drained while a command runs.
const OUTPUT_POLL_INTERVAL: Duration = Duration::from_millis(50);

impl CommandSticker {
    /// Arm the sticker: run it now, put it on its schedule, or both.
    ///
    /// `is_restore` is set when the window is merely reopening an already armed sticker, in which
    /// case "run immediately" must not fire again.
    pub(super) fn start(&mut self, window: &Window, cx: &mut Context<Self>, is_restore: bool) {
        self.open_in_settings = false;
        self.started_at = Some(crate::utils::time::now_unix_millis());

        let _ = self.save_config(cx);

        if self.is_schedule_active() {
            self.stop_schedule();
        }

        let content = self.build_content(cx);

        let Some(Scheduler::Cron(_)) = content.scheduler else {
            self.run(window, cx);
            return;
        };

        let Some(expr) = schedule::cron_expr(&content) else {
            self.fail("Cron expression cannot be empty", cx);
            return;
        };

        let parsed = match schedule::parse(expr) {
            Ok(parsed) => parsed,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };

        if self.run_immediately && !is_restore {
            self.run(window, cx);
        }

        let cancel = schedule::CancelToken::new();
        self.error = None;
        self.schedule_cancel = Some(cancel.clone());

        // Weak, deliberately: a strong handle here would outlive the window and keep the sticker
        // (and its claim on the schedule) alive forever, locking the background scheduler out.
        let entity = cx.entity().downgrade();
        window
            .spawn(cx, async move |window| {
                while !cancel.is_cancelled() {
                    let Some(sticker) = entity.upgrade() else {
                        // The window is gone; the background scheduler takes it from here.
                        break;
                    };

                    let now = chrono::Local::now();
                    let Some(next) = schedule::next_after(&parsed, now) else {
                        window.update_entity(&sticker, |this, _| this.stop_schedule());
                        break;
                    };

                    window.update_entity(&sticker, |this, cx| {
                        this.next_scheduled_at = Some(schedule::format(next));
                        cx.notify();
                    });
                    drop(sticker);

                    // Signed math first: `next` can already be in the past on a busy machine.
                    let mut remaining_ms =
                        (next.timestamp_millis() - now.timestamp_millis()).max(0) as u64;

                    while remaining_ms > 0 && !cancel.is_cancelled() {
                        let chunk = Duration::from_millis(remaining_ms).min(MAX_SLEEP_CHUNK);
                        window.background_executor().timer(chunk).await;
                        remaining_ms = remaining_ms.saturating_sub(chunk.as_millis() as u64);
                    }

                    if cancel.is_cancelled() {
                        break;
                    }

                    let Some(sticker) = entity.upgrade() else {
                        break;
                    };

                    let _ = window.update_window_entity(&sticker, |this, window, cx| {
                        if this.process.is_none() && !this.stopping {
                            this.stop(cx);
                            this.run(window, cx);
                        }
                    });
                }
            })
            .detach();
    }

    /// Spawn the command once and stream its output into the view.
    pub(super) fn run(&mut self, window: &Window, cx: &mut Context<Self>) {
        let content = self.build_content(cx);

        let spec = match RunSpec::resolve(&content, self.selection.as_deref()) {
            Ok(spec) => spec,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };

        let child = match spec.spawn() {
            Ok(child) => child,
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };

        let (process, rx) = runner::pump(child);
        self.process = Some(process);
        self.run_guard = Some(activity::begin_run(self.id));
        cx.notify();

        self.prepare_result_for_run();
        self.stream_output(window, cx, rx);
    }

    /// Clear whatever the previous run left behind, according to the streaming mode.
    fn prepare_result_for_run(&mut self) {
        if self.stream_result {
            // Streaming appends to the result as lines arrive, so it has to start from nothing.
            self.result.clear();
            self.result_html_entity = None;
            self.result_file_entity = None;
            return;
        }

        // Batch mode keeps the old output on screen until the new output replaces it wholesale.
        // HTML and file results own a child view, which would flicker if torn down early.
        if !matches!(
            self.result,
            CommandResult::Html(_) | CommandResult::Source(_)
        ) {
            self.result_html_entity = None;
            self.result_file_entity = None;
        }
    }

    fn stream_output(
        &mut self,
        window: &Window,
        cx: &Context<Self>,
        rx: std::sync::mpsc::Receiver<CmdEvent>,
    ) {
        let entity = cx.entity();
        window
            .spawn(cx, async move |window| {
                window.background_executor().timer(OUTPUT_WARMUP).await;

                let mut buffered = String::new();
                let mut succeeded = false;

                loop {
                    match rx.try_recv() {
                        Ok(CmdEvent::Output(line) | CmdEvent::Error(line)) => {
                            window.update_entity(&entity, |this: &mut CommandSticker, cx| {
                                if this.stream_result {
                                    let mut text = this.result.value().cloned().unwrap_or_default();
                                    text.push_str(&line);
                                    text.push('\n');
                                    this.result.set(Some(text));
                                    cx.notify();
                                } else {
                                    buffered.push_str(&line);
                                    buffered.push('\n');
                                }
                            });
                        }
                        Ok(CmdEvent::Done { success }) => {
                            succeeded = success;
                            window.update_entity(&entity, |this: &mut CommandSticker, cx| {
                                if !this.stream_result {
                                    this.result.set(Some(std::mem::take(&mut buffered)));
                                    cx.notify();
                                }
                            });
                            break;
                        }
                        Err(TryRecvError::Empty) => {
                            window
                                .background_executor()
                                .timer(OUTPUT_POLL_INTERVAL)
                                .await;
                        }
                        Err(TryRecvError::Disconnected) => break,
                    }
                }

                let _ = window.update_window_entity(
                    &entity,
                    move |this: &mut CommandSticker, window, cx| {
                        this.finish_run(succeeded, window, cx);
                    },
                );
            })
            .detach();
    }

    /// Rebuild the result views, persist the output, and decide the window's fate.
    fn finish_run(&mut self, succeeded: bool, window: &mut Window, cx: &mut Context<Self>) {
        let stopped_by_user = self.stopping;
        self.process = None;
        self.run_guard = None;
        self.stopping = false;

        self.result_html_entity = match &self.result {
            CommandResult::Html(Some(html)) => Some(cx.new(|cx| {
                let mut view = SimpleWebView::new(html.as_str(), window, cx);
                view.set_bg(
                    Rgba {
                        a: 0.85,
                        ..self.color.bg()
                    },
                    cx,
                );
                view
            })),
            _ => None,
        };

        self.result_file_entity = match &self.result {
            CommandResult::Source(Some(source)) => Some(output::build_file_content(
                self.id,
                source,
                self.color,
                self.store.clone(),
                window,
                cx,
                self.sticker_events_tx.clone(),
            )),
            _ => None,
        };

        self.save_config(cx);
        cx.notify();

        if self.window_hidden {
            // Running without a window: only reveal it when the command failed, otherwise dispose
            // of the hidden window unless it keeps running on a schedule.
            if succeeded || stopped_by_user {
                if !self.is_schedule_active() {
                    cx.emit(CommandStickerWindowRequest::Close);
                }
            } else {
                self.fail("Command failed, check the output above".to_string(), cx);
            }
        } else if self.should_auto_close(succeeded, stopped_by_user) {
            cx.emit(CommandStickerWindowRequest::Close);
        }
    }

    fn should_auto_close(&self, succeeded: bool, stopped_by_user: bool) -> bool {
        self.auto_close && succeeded && !stopped_by_user && !self.is_schedule_active()
    }

    /// Kill the running command, if there is one, and disarm the sticker.
    pub(super) fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(process) = self.process.clone() else {
            cx.notify();
            return;
        };

        self.stopping = true;
        self.started_at = None;
        self.save_config(cx);
        cx.notify();

        runner::kill_detached(process);
    }

    pub(super) fn stop_schedule(&mut self) {
        if let Some(cancel) = self.schedule_cancel.take() {
            cancel.cancel();
        }
        self.next_scheduled_at = None;
    }
}
