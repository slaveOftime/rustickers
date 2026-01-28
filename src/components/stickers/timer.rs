use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, AppContext, Context, Empty, Entity, Size, Window, div,
    prelude::*, px, transparent_white,
};
use gpui_component::{
    IndexPath, Sizable, StyledExt,
    alert::Alert,
    button::Button,
    green_500, h_flex,
    input::{Input, InputState},
    select::{SearchableVec, Select, SelectState},
    v_flex,
};
use serde::{Deserialize, Serialize};

use crate::windows::StickerWindowEvent;
use crate::{components::IconName, storage::ArcStickerStore};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerContent {
    title: Option<String>,
    duration_secs: i32,
    start_info: Option<TimerStartInfo>,
}

impl Default for TimerContent {
    fn default() -> Self {
        Self {
            title: None,
            duration_secs: 0,
            start_info: None,
        }
    }
}

pub struct TimerSticker {
    id: i64,
    store: ArcStickerStore,
    sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,
    timer: TimerContent,

    title: Entity<InputState>,
    hours: Entity<SelectState<SearchableVec<String>>>,
    minutes: Entity<SelectState<SearchableVec<String>>>,
    seconds: Entity<SelectState<SearchableVec<String>>>,

    last_save_time_while_countdown: i64,

    is_just_finished: bool,

    error: Option<String>,
}

impl TimerSticker {
    pub fn new<T>(
        id: i64,
        store: ArcStickerStore,
        content: &str,
        window: &mut Window,
        cx: &mut Context<T>,
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

        Self {
            id,
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
            error: None,
        }
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
                println!(
                    "Failed to send title changed event for timer sticker {}: {:?}",
                    id, err
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

        self.save_timer_state(cx);
    }

    fn change_state(&mut self, cx: &mut Context<Self>, state: TimerState) {
        let remaining_secs = effective_remaining_secs(&self.timer) as i32;
        if let Some(start_info) = &mut self.timer.start_info {
            match (&start_info.state, state) {
                (TimerState::Paused, TimerState::Running) => {
                    start_info.started_at_ms = crate::utils::time::now_unix_millis();
                    start_info.state = TimerState::Running;
                }
                (TimerState::Finished, TimerState::Running) => {
                    start_info.started_at_ms = crate::utils::time::now_unix_millis();
                    start_info.state = TimerState::Running;
                    start_info.remaining_secs = self.timer.duration_secs;
                }
                (TimerState::Running, TimerState::Paused) => {
                    start_info.remaining_secs = remaining_secs;
                    start_info.state = TimerState::Paused;
                }
                (_, TimerState::Finished) => {
                    self.timer.start_info = None;
                }
                _ => { /* No state change */ }
            }

            self.save_timer_state(cx);
        }
    }

    fn spawn_for_beep(&self, cx: &Context<Self>) {
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
        })
        .detach();
    }

    fn spawn_for_timer(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |e, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs_f64(0.8))
                .await;
            let _ = e.update(cx, |this, cx| {
                let mut is_just_finished = false;
                let remaining_secs = effective_remaining_secs(&this.timer);
                if let Some(start_info) = &mut this.timer.start_info {
                    if matches!(start_info.state, TimerState::Finished | TimerState::Paused) {
                        return;
                    }

                    if remaining_secs <= 0 {
                        is_just_finished = true;
                        start_info.state = TimerState::Finished;
                        cx.activate(true);
                    }

                    cx.notify();
                }

                this.is_just_finished = is_just_finished;
                if is_just_finished {
                    this.spawn_for_beep(cx);
                }

                if remaining_secs <= 0
                    || crate::utils::time::now_unix_millis() - this.last_save_time_while_countdown
                        >= 3000
                {
                    this.save_timer_state(cx);
                    this.last_save_time_while_countdown = crate::utils::time::now_unix_millis();
                    println!("Timer sticker {} state saved.", this.id);
                }
            });
        })
        .detach();
    }

    fn setter_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .justify_center()
            .items_center()
            .p_2()
            .gap_3()
            .child(Input::new(&self.title).min_w(px(100.0)).max_w(px(200.0)))
            .child(
                h_flex()
                    .max_w(px(300.0))
                    .items_center()
                    .gap_2()
                    .child(Select::new(&self.hours))
                    .child(":")
                    .child(Select::new(&self.minutes))
                    .child(":")
                    .child(Select::new(&self.seconds)),
            )
            .child(
                Button::new("timer-start")
                    .icon(IconName::Play)
                    .bg(transparent_white())
                    .border_0()
                    .on_click(cx.listener(|s, _, _, cx| s.start(cx))),
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

        let mut view = v_flex()
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
            .child(div().text_2xl().font_bold().child(label));

        view = match start_info.state {
            TimerState::Running => view.when(window.is_window_hovered(), |view| {
                view.child(
                    Button::new("pause")
                        .icon(IconName::Pause)
                        .bg(transparent_white())
                        .border_0()
                        .on_click(
                            cx.listener(|s, _, _, cx| s.change_state(cx, TimerState::Paused)),
                        ),
                )
            }),
            TimerState::Paused => view.child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("reset")
                            .icon(IconName::Adjustments)
                            .bg(transparent_white())
                            .border_0()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.is_just_finished = false;
                                this.change_state(cx, TimerState::Finished)
                            })),
                    )
                    .child(
                        Button::new("resume")
                            .icon(IconName::Play)
                            .bg(transparent_white())
                            .border_0()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.is_just_finished = false;
                                this.change_state(cx, TimerState::Running)
                            })),
                    ),
            ),
            TimerState::Finished => view.when(window.is_window_hovered(), |view| {
                view.child(
                    Button::new("reset")
                        .icon(IconName::Adjustments)
                        .bg(transparent_white())
                        .border_0()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.is_just_finished = false;
                            this.change_state(cx, TimerState::Finished)
                        })),
                )
            }),
        };

        return view.into_any_element();
    }
}

impl Sticker for TimerSticker {
    fn save_on_close(&mut self, cx: &mut Context<Self>) -> bool {
        self.save_timer_state(cx)
    }

    fn min_window_size() -> gpui::Size<i32> {
        Size::new(200, 100)
    }

    fn default_window_size() -> gpui::Size<i32> {
        Size::new(300, 200)
    }
}

impl Render for TimerSticker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex().size_full();

        if let Some(start_info) = &mut self.timer.start_info {
            if let TimerState::Running = start_info.state {
                self.spawn_for_timer(cx);
            }
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
