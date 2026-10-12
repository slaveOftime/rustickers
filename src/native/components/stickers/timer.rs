use std::time::Duration;

use crate::gpui_component::{
    IndexPath, Sizable, StyledExt,
    alert::Alert,
    button::{Button, ButtonVariants},
    green_500, h_flex,
    input::{Input, InputState},
    select::{SearchableVec, Select, SelectState},
    v_flex,
};
use gpui::{
    Animation, AnimationExt, AnyElement, AppContext, Context, Empty, Entity, Size, Window,
    WindowControlArea, div, prelude::*, px,
};

/// Result of a single tick-loop iteration. `Continue(ms)` asks the loop
/// driver to sleep for at most `ms` milliseconds before the next body
/// call. `Stop` exits the loop on the next scheduling point.
enum TickBodyOutcome {
    Continue(u64),
    Stop,
}
use serde::{Deserialize, Serialize};

use crate::model::sticker::StickerColor;
use crate::native::{components::IconName, windows::StickerWindowEvent};
use crate::storage::ArcStickerStore;

use super::Sticker;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
enum TimerState {
    Running,
    Paused,
    Finished,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerStartInfo {
    started_at_ms: i64,
    remaining_secs: i32,
    state: TimerState,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TimerContent {
    title: Option<String>,
    duration_secs: i32,
    start_info: Option<TimerStartInfo>,
}

pub struct TimerSticker {
    id: i64,
    color: StickerColor,
    store: ArcStickerStore,
    sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,
    timer: TimerContent,

    title: Entity<InputState>,
    hours: Entity<SelectState<SearchableVec<String>>>,
    minutes: Entity<SelectState<SearchableVec<String>>>,
    seconds: Entity<SelectState<SearchableVec<String>>>,

    last_save_time_while_countdown: i64,

    is_just_finished: bool,
    /// Monotonic counter owned by this entity. Every tick-loop run is
    /// tagged with the value at the moment it was started; if the entity
    /// starts a new loop, a transition happens, or the entity is being
    /// shut down, this counter moves and the prior loop exits on its
    /// next iteration. The previous `bool` only proved *some* loop was
    /// running; the revision tag proves *this exact loop is still*
    /// wanted.
    tick_loop_revision: u64,
    last_displayed_remaining_secs: i32,

    error: Option<String>,
}

impl TimerSticker {
    pub fn new(
        id: i64,
        color: StickerColor,
        store: ArcStickerStore,
        content: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,
    ) -> Self {
        let timer = parse_content(content);
        let title = timer.title.clone().unwrap_or("".to_string());
        let (h, m, s) = crate::utils::time::secs_to_hms(timer.duration_secs.max(0) as i64);

        let hours = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new((0..24).map(|x| format!("{:02}", x)).collect::<Vec<_>>()),
                Some(IndexPath::default().row(h as usize)),
                window,
                cx,
            )
            .searchable(true)
        });

        let minutes = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new((0..60).map(|x| format!("{:02}", x)).collect::<Vec<_>>()),
                Some(IndexPath::default().row(m as usize)),
                window,
                cx,
            )
            .searchable(true)
        });

        let seconds = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new((0..60).map(|x| format!("{:02}", x)).collect::<Vec<_>>()),
                Some(IndexPath::default().row(s as usize)),
                window,
                cx,
            )
            .searchable(true)
        });

        let mut sticker = Self {
            id,
            color,
            store,
            sticker_events_tx,
            timer,
            title: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(title)
                    .placeholder("Give some title or hint")
            }),
            hours,
            minutes,
            seconds,
            last_save_time_while_countdown: 0,
            is_just_finished: false,
            tick_loop_revision: 0,
            last_displayed_remaining_secs: -1,
            error: None,
        };

        // Restore a tick loop when a running timer is reopened. Without
        // this, a sticker that's ticking while its window is being
        // restored would only show a countdown frame after the next
        // render. begin_tick_loop checks the current revision, not just
        // the bool flag, so it's safe to call unconditionally here.
        if matches!(
            sticker.timer.start_info.as_ref().map(|s| &s.state),
            Some(TimerState::Running)
        ) {
            sticker.begin_tick_loop(cx);
        }

        sticker
    }

    fn save_timer_state(&mut self, cx: &mut Context<Self>) -> bool {
        let (h, m, s) = crate::utils::time::secs_to_hms(self.timer.duration_secs as i64);

        let title = self.title.read(cx).value().to_string();
        let title = if title.is_empty() {
            format!("{:02}h {:02}m {:02}s", h, m, s)
        } else {
            title
        };

        let json = match serde_json::to_string(&self.timer) {
            Ok(json) => json,
            Err(err) => {
                self.error = Some(format!("Failed to save timer state: {}", err));
                return false;
            }
        };

        let store = self.store.clone();
        let sticker_events_tx = self.sticker_events_tx.clone();
        let id = self.id;

        self.error = None;

        cx.spawn(async move |entity, cx| {
            if let Err(err) = store.update_sticker_title(id, title.clone()).await {
                let _ = entity.update(cx, |this, cx| {
                    this.error = Some(format!("Failed to save timer title: {:?}", err));
                    cx.notify();
                });
                return;
            }

            if let Err(err) = sticker_events_tx.send(StickerWindowEvent::TitleChanged { id, title })
            {
                tracing::warn!(
                    id,
                    error = %err,
                    "Failed to send title changed event for timer sticker"
                );
            }

            if let Err(err) = store.update_sticker_content(id, json).await {
                let _ = entity.update(cx, |this, cx| {
                    this.error = Some(format!("Failed to save timer state: {:?}", err));
                    cx.notify();
                });
                return;
            }

            let _ = entity.update(cx, |this, cx| {
                this.error = None;
                cx.notify();
            });
        })
        .detach();

        true
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let h = self
            .hours
            .read(cx)
            .selected_value()
            .and_then(|x| x.parse::<i32>().ok())
            .unwrap_or(0);
        let m = self
            .minutes
            .read(cx)
            .selected_value()
            .and_then(|x| x.parse::<i32>().ok())
            .unwrap_or(0);
        let s = self
            .seconds
            .read(cx)
            .selected_value()
            .and_then(|x| x.parse::<i32>().ok())
            .unwrap_or(0);

        let duration_secs = (h.max(0) * 3600) + (m.max(0) * 60) + s.max(0);
        if duration_secs <= 0 {
            self.error = Some("Duration must be greater than zero.".to_string());
            cx.notify();
            return;
        }

        self.timer = TimerContent {
            title: Some(self.title.read(cx).value().to_string()),
            duration_secs,
            start_info: Some(TimerStartInfo {
                started_at_ms: crate::utils::time::now_unix_millis(),
                remaining_secs: duration_secs,
                state: TimerState::Running,
            }),
        };
        self.last_displayed_remaining_secs = -1;
        self.supersede_tick_loop();

        self.save_timer_state(cx);
        // Initialise the tick loop *now* so the freshly-started countdown
        // does not depend on the next render pass noticing the running state.
        self.begin_tick_loop(cx);
    }

    fn change_state(&mut self, cx: &mut Context<Self>, state: TimerState) {
        let remaining_secs = effective_remaining_secs(&self.timer);
        let mut begin_tick = false;
        if let Some(start_info) = &mut self.timer.start_info {
            match (&start_info.state, state) {
                (TimerState::Paused, TimerState::Running) => {
                    start_info.started_at_ms = crate::utils::time::now_unix_millis();
                    start_info.state = TimerState::Running;
                    self.last_displayed_remaining_secs = -1;
                    begin_tick = true;
                }
                (TimerState::Finished, TimerState::Running) => {
                    start_info.started_at_ms = crate::utils::time::now_unix_millis();
                    start_info.state = TimerState::Running;
                    start_info.remaining_secs = self.timer.duration_secs;
                    self.last_displayed_remaining_secs = -1;
                    begin_tick = true;
                }
                (TimerState::Running, TimerState::Paused) => {
                    start_info.remaining_secs = remaining_secs;
                    start_info.state = TimerState::Paused;
                    // No tick loop; supersede whatever is currently running.
                    self.supersede_tick_loop();
                }
                (_, TimerState::Finished) => {
                    self.timer.start_info = None;
                    self.supersede_tick_loop();
                    self.last_displayed_remaining_secs = -1;
                }
                _ => { /* No state change */ }
            }

            self.save_timer_state(cx);
        }

        if begin_tick {
            self.begin_tick_loop(cx);
        }
    }

    fn restart(&mut self, cx: &mut Context<Self>) {
        self.is_just_finished = false;
        let mut begin_tick = false;

        if let Some(start_info) = &mut self.timer.start_info {
            start_info.started_at_ms = crate::utils::time::now_unix_millis();
            start_info.remaining_secs = self.timer.duration_secs;
            start_info.state = TimerState::Running;
            self.last_displayed_remaining_secs = -1;
            begin_tick = true;

            self.save_timer_state(cx);
        }

        if begin_tick {
            self.begin_tick_loop(cx);
        }
    }

    fn spawn_for_beep(&self, cx: &mut Context<Self>) {
        cx.spawn(async |this, cx| {
            let start = crate::utils::time::now_unix_millis();
            loop {
                if crate::utils::time::now_unix_millis() - start < 10000
                    && let Ok(true) = this.read_with(cx, |this, _| this.is_just_finished)
                {
                    play_beep();
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                } else {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.is_just_finished = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Invalidate any currently-running tick loop on this entity. Detached
    /// loops check `tick_loop_revision` against the snapshot they captured
    /// at start; once it changes they exit on the next iteration. Safe to
    /// call from any state transition, including ones the loop will
    /// observe; supersession is monotonic.
    fn supersede_tick_loop(&mut self) {
        self.tick_loop_revision = self.tick_loop_revision.saturating_add(1);
    }

    /// Start a tick loop pinned to the current revision. Tasks perform one
    /// body call per iteration, then sleep until the next second boundary
    /// or 200 ms, whichever is sooner. They exit when:
    ///
    /// - the controller moves to a non-Running state,
    /// - the controller bumps `tick_loop_revision` (start/restart/resume),
    /// - or `update` returns Err (entity dropped).
    fn begin_tick_loop(&mut self, cx: &mut Context<Self>) {
        self.supersede_tick_loop();
        let revision = self.tick_loop_revision;

        cx.spawn(async move |entity, cx| {
            loop {
                let sleep_ms: u64 = match entity.update(cx, |this, cx| {
                    if this.tick_loop_revision != revision {
                        return None;
                    }
                    match this.tick_body(cx) {
                        TickBodyOutcome::Continue(until_next_ms) => Some(until_next_ms),
                        TickBodyOutcome::Stop => None,
                    }
                }) {
                    Ok(Some(until_next_ms)) => until_next_ms,
                    Ok(None) | Err(_) => break,
                };

                cx.background_executor()
                    .timer(Duration::from_millis(sleep_ms))
                    .await;
            }
        })
        .detach();
    }

    /// One iteration of the tick loop. Caller must have already validated
    /// that `tick_loop_revision == revision`. Returns the duration until
    /// the loop should wake again, or `Stop` to terminate the loop.
    fn tick_body(&mut self, cx: &mut Context<Self>) -> TickBodyOutcome {
        let Some(start_info) = self.timer.start_info.as_ref() else {
            // State was cleared underneath us. Leave the loop body; the
            // loop driver will exit because there is no next wake to schedule.
            // supersession isn't necessary: the next start() / restart()
            // will bump the revision, and `change_state(_, Finished)` already
            // did so before clearing `start_info`.
            return TickBodyOutcome::Stop;
        };

        if !matches!(start_info.state, TimerState::Running) {
            return TickBodyOutcome::Stop;
        }

        let now_ms = crate::utils::time::now_unix_millis();
        let elapsed_ms = now_ms.saturating_sub(start_info.started_at_ms);
        let remaining_ms = ((start_info.remaining_secs as i64) * 1000 - elapsed_ms).max(0);
        let remaining_secs = (start_info.remaining_secs - (elapsed_ms / 1000) as i32).max(0);

        if remaining_secs != self.last_displayed_remaining_secs {
            self.last_displayed_remaining_secs = remaining_secs;
            cx.notify();
        }

        if remaining_secs <= 0 {
            self.is_just_finished = true;
            if let Some(start_info) = &mut self.timer.start_info {
                start_info.state = TimerState::Finished;
            }
            // The state has flipped terminal; bump the revision so the
            // loop driver knows to exit even though this iteration may
            // still schedule its first sleep.
            self.supersede_tick_loop();
            cx.activate(true);
            self.spawn_for_beep(cx);
            return TickBodyOutcome::Stop;
        }

        if now_ms - self.last_save_time_while_countdown >= 3000 {
            self.save_timer_state(cx);
            self.last_save_time_while_countdown = now_ms;
            tracing::trace!(id = self.id, "Timer sticker state saved");
        }

        let until_next_second = (remaining_ms % 1000).max(1) as u64;
        TickBodyOutcome::Continue(until_next_second.min(200))
    }

    fn setter_view(&mut self, _cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .justify_center()
            .items_center()
            .p_2()
            .gap_3()
            .child(Input::new(&self.title).small().w(px(160.0)))
            .child(
                h_flex()
                    .max_w(px(300.0))
                    .items_center()
                    .gap_2()
                    .occlude()
                    .child(Select::new(&self.hours).small())
                    .child(":")
                    .child(Select::new(&self.minutes).small())
                    .child(":")
                    .child(Select::new(&self.seconds).small()),
            )
            .into_any_element()
    }

    fn countdown_view(&mut self, cx: &mut Context<Self>, window: &mut Window) -> AnyElement {
        let Some(start_info) = &self.timer.start_info else {
            return Empty.into_any_element();
        };

        let remaining_secs = match start_info.state {
            TimerState::Running => effective_remaining_secs(&self.timer),
            TimerState::Paused => start_info.remaining_secs,
            TimerState::Finished => 0,
        };

        let title = self.title.read(cx).value();

        let (h, m, s) = crate::utils::time::secs_to_hms(remaining_secs as i64);
        let label = format!("{:02} : {:02} : {:02}", h, m, s);

        let percentage = if self.timer.duration_secs > 0 {
            let elapsed_secs = self.timer.duration_secs.saturating_sub(remaining_secs);
            elapsed_secs as f64 / self.timer.duration_secs as f64
        } else {
            0.0
        };

        v_flex()
            .size_full()
            .p_3()
            .gap_1()
            .items_center()
            .justify_center()
            .relative()
            .when(!self.is_just_finished, |view| {
                view.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .bg(green_500().opacity(0.1))
                        .w(px((window.bounds().size.width.to_f64() * percentage) as f32)),
                )
            })
            .when(self.is_just_finished, |view| {
                view.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .bg(green_500())
                        .right_0()
                        .with_animation(
                            "indicator",
                            Animation::new(Duration::from_millis(800)).repeat(),
                            |v, x| v.opacity(0.3 * x),
                        ),
                )
            })
            .when(!title.is_empty(), |view| view.child(title))
            .child(div().text_2xl().font_bold().child(label))
            .into_any_element()
    }
}

impl Sticker for TimerSticker {
    fn id(&self) -> i64 {
        self.id
    }

    fn save_on_close(&mut self, cx: &mut Context<Self>) -> bool {
        self.save_timer_state(cx)
    }

    fn min_window_size() -> gpui::Size<i32> {
        Size::new(200, 100)
    }

    fn default_window_size() -> gpui::Size<i32> {
        Size::new(300, 200)
    }

    fn set_color(&mut self, color: StickerColor) {
        self.color = color;
    }

    fn footer_extension(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(state) = self
            .timer
            .start_info
            .as_ref()
            .map(|start_info| start_info.state.clone())
        else {
            return Some(
                Button::new("timer-start")
                    .icon(IconName::Play)
                    .ghost()
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| this.start(cx)))
                    .into_any_element(),
            );
        };

        let controls = h_flex().gap_1();
        Some(
            match state {
                TimerState::Running => controls.child(
                    Button::new("pause")
                        .icon(IconName::Pause)
                        .ghost()
                        .occlude()
                        .on_click(
                            cx.listener(|this, _, _, cx| this.change_state(cx, TimerState::Paused)),
                        ),
                ),
                TimerState::Paused => controls
                    .child(
                        Button::new("reset")
                            .icon(IconName::Adjustments)
                            .ghost()
                            .occlude()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.is_just_finished = false;
                                this.change_state(cx, TimerState::Finished)
                            })),
                    )
                    .child(
                        Button::new("resume")
                            .icon(IconName::Play)
                            .ghost()
                            .occlude()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.is_just_finished = false;
                                this.change_state(cx, TimerState::Running)
                            })),
                    ),
                TimerState::Finished => controls
                    .child(
                        Button::new("reset")
                            .icon(IconName::Adjustments)
                            .ghost()
                            .occlude()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.is_just_finished = false;
                                this.change_state(cx, TimerState::Finished)
                            })),
                    )
                    .child(
                        Button::new("restart")
                            .icon(IconName::Forward)
                            .ghost()
                            .occlude()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.restart(cx);
                            })),
                    ),
            }
            .into_any_element(),
        )
    }
}

impl Render for TimerSticker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex()
            .size_full()
            .window_control_area(WindowControlArea::Drag);

        if let Some(start_info) = &mut self.timer.start_info {
            // Render must be a pure description of state. The tick loop
            // is started in `start()`, `change_state(_, Running)` and
            // `restart()`; this branch only reflects that state to the
            // user.
            let _ = start_info;
            body = body.child(self.countdown_view(cx, window));
        } else {
            body = body.child(self.setter_view(cx));
        }

        if let Some(err) = &self.error {
            body = body.child(Alert::error("timer-error", err.as_str()).small());
        }

        body.into_any_element()
    }
}

fn parse_content(content: &str) -> TimerContent {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return TimerContent::default();
    }
    serde_json::from_str::<TimerContent>(trimmed).unwrap_or_default()
}

fn effective_remaining_secs(timer: &TimerContent) -> i32 {
    if let Some(start_info) = &timer.start_info {
        let elapsed_ms =
            crate::utils::time::now_unix_millis().saturating_sub(start_info.started_at_ms);
        let elapsed_secs = elapsed_ms / 1000;
        let remaining_secs = start_info.remaining_secs - elapsed_secs as i32;
        remaining_secs.max(0)
    } else {
        timer.duration_secs.max(0)
    }
}

fn play_beep() {
    #[cfg(windows)]
    unsafe {
        // Beep(frequency_hz, duration_ms)
        let _ = windows_sys::Win32::System::Diagnostics::Debug::Beep(880, 200);
    }

    #[cfg(not(windows))]
    {
        // Best-effort fallback: terminal bell.
        use std::io::Write;
        let _ = std::io::stdout().write_all(b"\x07");
        let _ = std::io::stdout().flush();
    }
}
