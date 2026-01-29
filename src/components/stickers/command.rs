use gpui::{
    Animation, AnimationExt, AnyElement, AppContext, Context, Entity, Image, ImageFormat,
    ImageSource, Render, Rgba, Window, div, img, prelude::*, px, transparent_white,
};
use gpui_component::{
    Sizable,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    form::{field, v_form},
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement,
    switch::Switch,
    text::TextView,
    v_flex, yellow_500,
};
use serde::{Deserialize, Serialize};
use std::{
    process::{Command, Stdio},
    str::FromStr,
    sync::atomic::{AtomicBool, Ordering},
    sync::mpsc::{self, TryRecvError},
    sync::{Arc, Mutex, RwLock},
    thread,
    time::Duration,
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use crate::{
    components::IconName, components::webview::SimpleWebView, model::sticker::StickerColor,
    storage::ArcStickerStore, windows::StickerWindowEvent,
};

const MAX_SLEEP_CHUNK_MS: u64 = 250;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CommandContent {
    command: String,
    environments: String,
    working_dir: String,
    scheduler: Option<Scheduler>,
    run_immediately: bool,
    result: CommandResult,
    stream_result: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CommandResult {
    Text(Option<String>),
    Html(Option<String>),
    Svg(Option<String>),
    Markdown(Option<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Scheduler {
    Cron(String),
}

impl Default for CommandContent {
    fn default() -> Self {
        Self {
            command: String::new(),
            environments: String::new(),
            working_dir: String::new(),
            scheduler: None,
            run_immediately: true,
            stream_result: false,
            result: CommandResult::Text(None),
        }
    }
}

pub struct CommandSticker {
    id: i64,
    color: StickerColor,
    store: ArcStickerStore,
    sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,

    command: Entity<InputState>,
    environments: Entity<InputState>,
    working_dir: Entity<InputState>,
    scheduler: Option<Scheduler>,
    scheduler_cron_input: Entity<InputState>,
    run_immediately: bool,
    stream_result: bool,

    result: CommandResult,
    result_html_entity: Option<Entity<SimpleWebView>>,

    process: Option<Arc<Mutex<std::process::Child>>>,
    stopping: bool,

    schedule_cancel: Option<Arc<AtomicBool>>,
    next_scheduled_at: Option<String>,
    error: Option<String>,
}

enum CmdEvent {
    Output(String),
    Error(String),
    Done,
}

impl CommandSticker {
    pub fn new(
        id: i64,
        color: StickerColor,
        store: ArcStickerStore,
        content: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,
    ) -> Self {
        let cmd = serde_json::from_str::<CommandContent>(content).unwrap_or_default();
        let command_value = cmd.command;
        let envs_value = cmd.environments;
        let workdir_value = cmd.working_dir;

        let command = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(command_value)
                .placeholder("command with args")
        });

        let environments = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .auto_grow(1, 10)
                .default_value(envs_value)
                .placeholder("KEY=VALUE per line")
        });

        let working_dir = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(workdir_value)
                .placeholder("Optional")
        });

        let cron = match &cmd.scheduler {
            Some(Scheduler::Cron(cron)) => cron.clone(),
            _ => String::new(),
        };
        let cron_entity = cx.new(|cx| InputState::new(window, cx).default_value(cron));

        let result_html_entity = match &cmd.result {
            CommandResult::Html(Some(x)) => {
                Some(cx.new(|cx| SimpleWebView::new(x.as_str(), window, cx)))
            }
            _ => None,
        };

        cx.subscribe(&cron_entity, |this, v, evt, cx| match evt {
            InputEvent::Change => {
                this.scheduler = Some(Scheduler::Cron(v.read(cx).value().trim().to_string()));
            }
            _ => {}
        })
        .detach();

        Self {
            id,
            color,
            store,
            sticker_events_tx,

            command,
            environments,
            working_dir,
            scheduler: cmd.scheduler,
            scheduler_cron_input: cron_entity,
            run_immediately: cmd.run_immediately,
            result: cmd.result,
            result_html_entity,
            stream_result: cmd.stream_result,

            process: None,
            stopping: false,

            schedule_cancel: None,
            next_scheduled_at: None,
            error: None,
        }
    }

    fn build_content(&self, cx: &mut Context<Self>) -> CommandContent {
        CommandContent {
            command: self.command.read(cx).value().trim().to_string(),
            environments: self.environments.read(cx).value().to_string(),
            working_dir: self.working_dir.read(cx).value().to_string(),
            scheduler: self.scheduler.clone(),
            run_immediately: self.run_immediately,
            result: self.result.clone(),
            stream_result: self.stream_result,
        }
    }

    fn save_config(&mut self, cx: &mut Context<Self>) -> bool {
        let content = self.build_content(cx);
        let title = content.command.trim().to_string();
        let json = match serde_json::to_string(&content) {
            Ok(json) => json,
            Err(err) => {
                self.error = Some(format!("Failed to serialize command sticker: {err}"));
                return false;
            }
        };

        let id = self.id;
        let store = self.store.clone();
        let sticker_events_tx = self.sticker_events_tx.clone();

        cx.spawn(async move |entity, cx| {
            if let Err(err) = store.update_sticker_title(id, title.clone()).await {
                let _ = entity.update(cx, |this, cx| {
                    this.error = Some(format!("Failed to save command sticker title: {err:#}"));
                    cx.notify();
                });
                return;
            }

            if let Err(err) = sticker_events_tx.send(StickerWindowEvent::TitleChanged { id, title })
            {
                tracing::warn!(id, error = %err, "Failed to send sticker title changed event");
            }

            if let Err(err) = store.update_sticker_content(id, json).await {
                let _ = entity.update(cx, |this, cx| {
                    this.error = Some(format!("Failed to save command sticker: {err:#}"));
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

    fn is_schedule_active(&self) -> bool {
        self.schedule_cancel.is_some()
    }

    fn start(&mut self, window: &Window, cx: &mut Context<Self>) {
        let _ = self.save_config(cx);

        if self.is_schedule_active() {
            self.stop_schedule();
        }

        let content = self.build_content(cx);
        match content.scheduler.clone() {
            None => {
                self.run(window, cx);
            }
            Some(Scheduler::Cron(expr)) => {
                if expr.is_empty() {
                    self.error = Some("Cron expression cannot be empty".to_string());
                    cx.notify();
                    return;
                }

                let schedule = match cron::Schedule::from_str(&expr) {
                    Ok(s) => s,
                    Err(err) => {
                        self.error = Some(format!("Invalid cron expression: {err}"));
                        cx.notify();
                        return;
                    }
                };

                if self.run_immediately {
                    self.run(window, cx);
                }

                let cancel = Arc::new(AtomicBool::new(false));

                self.error = None;
                self.schedule_cancel = Some(cancel.clone());

                let entity = cx.entity();
                window
                    .spawn(cx, async move |window| {
                        loop {
                            if cancel.load(Ordering::SeqCst) {
                                break;
                            }

                            let now = chrono::Local::now();
                            let next = schedule.upcoming(chrono::Local).next();
                            let Some(next) = next else {
                                let _ =
                                    window.update_entity(&entity, |this, _| this.stop_schedule());
                                break;
                            };

                            let next_str = next.format("%Y-%m-%d %H:%M:%S").to_string();
                            let _ = window.update_entity(&entity, |this, cx| {
                                this.next_scheduled_at = Some(next_str);
                                cx.notify();
                            });

                            // Compute delay with signed math first to avoid underflow when
                            // `next` is already in the past.
                            let delay_ms_i64 = next.timestamp_millis() - now.timestamp_millis();
                            if delay_ms_i64 <= 0 {
                                let _ = window.update_window_entity(&entity, |this, window, cx| {
                                    if this.process.is_none() && !this.stopping {
                                        this.stop(cx);
                                        this.run(window, cx);
                                    }
                                });
                                continue;
                            }

                            // Make the wait cancellable: instead of awaiting one long timer (which
                            // can't be interrupted), sleep in small chunks and check `cancel`.
                            let mut remaining_ms = delay_ms_i64 as u64;
                            while remaining_ms > 0 {
                                if cancel.load(Ordering::SeqCst) {
                                    break;
                                }
                                let chunk = remaining_ms.min(MAX_SLEEP_CHUNK_MS);
                                window
                                    .background_executor()
                                    .timer(Duration::from_millis(chunk))
                                    .await;
                                remaining_ms = remaining_ms.saturating_sub(chunk);
                            }

                            if cancel.load(Ordering::SeqCst) {
                                break;
                            }

                            let _ = window.update_window_entity(&entity, |this, window, cx| {
                                if this.process.is_none() && !this.stopping {
                                    this.stop(cx);
                                    this.run(window, cx);
                                }
                            });
                        }
                    })
                    .detach();
            }
        }
    }

    fn run(&mut self, window: &Window, cx: &mut Context<Self>) {
        let content = self.build_content(cx);
        if content.command.trim().is_empty() {
            self.error = Some("Command cannot be empty".to_string());
            cx.notify();
            return;
        }

        let mut args = winsplit::split(&content.command);
        if args.is_empty() {
            self.error = Some("Command cannot be empty".to_string());
            cx.notify();
            return;
        }

        let workdir = content.working_dir.trim();

        let program = args.remove(0);
        let Ok(path) = which::which(&program) else {
            self.error = Some(format!("Command not found: {}", program));
            cx.notify();
            return;
        };

        let mut cmd = Command::new(path);

        #[cfg(target_os = "windows")]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        if !args.is_empty() {
            cmd.args(args);
        }

        if !workdir.is_empty() {
            cmd.current_dir(workdir);
        }

        for env in content.environments.lines() {
            let line = env.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                cmd.env(k.trim(), v.trim());
            } else {
                cmd.env(line, "");
            }
        }

        let process = match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
            Ok(c) => c,
            Err(err) => {
                self.error = Some(format!("Failed to start command: {err}"));
                cx.notify();
                return;
            }
        };

        let (tx, rx) = mpsc::channel();
        self.handle_stdout_and_err(cx, tx, process);
        self.handle_cmd_events(window, cx, rx);
    }

    fn handle_stdout_and_err(
        &mut self,
        cx: &mut Context<Self>,
        tx: mpsc::Sender<CmdEvent>,
        mut process: std::process::Child,
    ) {
        let stdout = process.stdout.take();
        let stderr = process.stderr.take();
        let process = Arc::new(Mutex::new(process));

        self.process = Some(process.clone());
        cx.notify();

        thread::spawn(move || {
            let out_tx = tx.clone();
            let out_handle = thread::spawn(move || {
                if let Some(stdout) = stdout {
                    let reader = std::io::BufReader::new(stdout);
                    for line in std::io::BufRead::lines(reader).flatten() {
                        let _ = out_tx.send(CmdEvent::Output(line));
                    }
                }
            });

            let err_tx = tx.clone();
            let err_handle = thread::spawn(move || {
                if let Some(stderr) = stderr {
                    let reader = std::io::BufReader::new(stderr);
                    for line in std::io::BufRead::lines(reader).flatten() {
                        let _ = err_tx.send(CmdEvent::Error(line));
                    }
                }
            });

            // IMPORTANT: do not hold the mutex while waiting. If we call `wait()` while
            // holding the lock, `stop()` cannot lock the child to kill it.
            loop {
                let is_done = match process.lock() {
                    Ok(mut child) => match child.try_wait() {
                        Ok(Some(_status)) => true,
                        Ok(None) => false,
                        Err(_err) => true,
                    },
                    Err(_err) => true,
                };

                if is_done {
                    break;
                }

                thread::sleep(Duration::from_millis(50));
            }

            let _ = tx.send(CmdEvent::Done);
            let _ = out_handle.join();
            let _ = err_handle.join();
        });
    }

    fn handle_cmd_events(
        &mut self,
        window: &Window,
        cx: &Context<Self>,
        rx: mpsc::Receiver<CmdEvent>,
    ) {
        if self.stream_result {
            match self.result {
                CommandResult::Text(ref mut result)
                | CommandResult::Markdown(ref mut result)
                | CommandResult::Html(ref mut result)
                | CommandResult::Svg(ref mut result) => {
                    *result = None;
                }
            }
            self.result_html_entity = None;
        } else {
            match &self.result {
                CommandResult::Html(_) => {}
                _ => {
                    self.result_html_entity = None;
                }
            };
        }

        let entity = cx.entity();
        window
            .spawn(cx, async move |window| {
                window
                    .background_executor()
                    .timer(Duration::from_millis(100))
                    .await;

                let result_temp = Arc::new(RwLock::new(String::new()));
                loop {
                    let result_temp = result_temp.clone();
                    match rx.try_recv() {
                        Ok(event) => match event {
                            CmdEvent::Output(line) | CmdEvent::Error(line) => {
                                let _ = window.update_entity(
                                    &entity,
                                    move |this: &mut CommandSticker, cx| {
                                        match this.result {
                                            CommandResult::Text(ref mut result)
                                            | CommandResult::Markdown(ref mut result) => {
                                                let result = result.get_or_insert_with(String::new);
                                                result.push_str(&line);
                                                result.push('\n');
                                            }
                                            CommandResult::Html(_) | CommandResult::Svg(_) => {
                                                *result_temp.write().unwrap() += &line;
                                                *result_temp.write().unwrap() += "\n";
                                            }
                                        }
                                        cx.notify();
                                    },
                                );
                            }
                            CmdEvent::Done => {
                                let _ = window.update_entity(
                                    &entity,
                                    move |this: &mut CommandSticker, _| match this.result {
                                        CommandResult::Text(_) | CommandResult::Markdown(_) => {}
                                        CommandResult::Html(ref mut result)
                                        | CommandResult::Svg(ref mut result) => {
                                            *result = Some(result_temp.read().unwrap().clone());
                                        }
                                    },
                                );
                                break;
                            }
                        },
                        Err(TryRecvError::Empty) => {
                            window
                                .background_executor()
                                .timer(Duration::from_millis(50))
                                .await;
                        }
                        Err(TryRecvError::Disconnected) => {
                            break;
                        }
                    }
                }

                let _ = window.update_window_entity(
                    &entity,
                    move |this: &mut CommandSticker, window, cx| {
                        this.process = None;
                        this.stopping = false;
                        this.result_html_entity = match &this.result {
                            CommandResult::Html(Some(x)) => {
                                Some(cx.new(|cx| SimpleWebView::new(x.as_str(), window, cx)))
                            }
                            _ => None,
                        };
                        this.save_config(cx);
                        cx.notify();
                    },
                );
            })
            .detach();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(process) = self.process.as_ref().map(|x| x.clone()) else {
            cx.notify();
            return;
        };

        self.stopping = true;
        self.save_config(cx);
        cx.notify();

        thread::spawn(move || {
            match process.lock() {
                Ok(mut process) => {
                    kill_process(&mut process);
                }
                Err(err) => {
                    tracing::warn!(error = %err, "CommandSticker: failed to lock process for killing");
                }
            };
        });
    }

    fn stop_schedule(&mut self) {
        if let Some(cancel) = self.schedule_cancel.take() {
            cancel.store(true, Ordering::SeqCst);
        }
        self.next_scheduled_at = None;
    }

    fn form(&mut self, cx: &mut Context<Self>) -> AnyElement {
        v_form()
            .child(field().label("Command").child(Input::new(&self.command)))
            .child(
                field().label("Render output as").child(
                    h_flex()
                        .gap_1()
                        .flex_wrap()
                        .child(
                            Button::new("text")
                                .label("text")
                                .small()
                                .when(
                                    match self.result {
                                        CommandResult::Text(_) => true,
                                        _ => false,
                                    },
                                    |v| v.primary(),
                                )
                                .on_click(cx.listener(|this, _, _, _| {
                                    this.result = CommandResult::Text(None)
                                })),
                        )
                        .child(
                            Button::new("markdown")
                                .label("markdown")
                                .small()
                                .when(
                                    match self.result {
                                        CommandResult::Markdown(_) => true,
                                        _ => false,
                                    },
                                    |v| v.primary(),
                                )
                                .on_click(cx.listener(|this, _, _, _| {
                                    this.result = CommandResult::Markdown(None)
                                })),
                        )
                        .child(
                            Button::new("html")
                                .label("html")
                                .small()
                                .when(
                                    match self.result {
                                        CommandResult::Html(_) => true,
                                        _ => false,
                                    },
                                    |v| v.primary(),
                                )
                                .on_click(cx.listener(|this, _, _, _| {
                                    this.result = CommandResult::Html(None)
                                })),
                        )
                        .child(
                            Button::new("svg")
                                .label("svg")
                                .small()
                                .when(
                                    match self.result {
                                        CommandResult::Svg(_) => true,
                                        _ => false,
                                    },
                                    |v| v.primary(),
                                )
                                .on_click(cx.listener(|this, _, _, _| {
                                    this.result = CommandResult::Svg(None)
                                })),
                        ),
                ),
            )
            .child(
                field().label("Stream output").child(
                    Switch::new("stream_output")
                        .label("will clean old result when running")
                        .small()
                        .checked(self.stream_result)
                        .on_click(
                            cx.listener(|this, _, _, _| this.stream_result = !this.stream_result),
                        ),
                ),
            )
            .child(
                field().label("Schedule").child(
                    v_flex()
                        .py_1()
                        .w_full()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_1()
                                .flex_wrap()
                                .child(
                                    Button::new("none")
                                        .label("none")
                                        .small()
                                        .when(self.scheduler.is_none(), |v| v.primary())
                                        .on_click(cx.listener(|this, _, _, _| {
                                            this.scheduler = None;
                                        })),
                                )
                                .child(
                                    Button::new("cron")
                                        .label("cron")
                                        .small()
                                        .when(
                                            matches!(self.scheduler, Some(Scheduler::Cron(_))),
                                            |v| v.primary(),
                                        )
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            // by default, every one minute
                                            let cron = "0 */1 * * * *";
                                            this.scheduler_cron_input.update(cx, |this, cx| {
                                                this.set_value(cron, window, cx)
                                            });
                                        })),
                                ),
                        )
                        .when(matches!(self.scheduler, Some(Scheduler::Cron(_))), |v| {
                            v.child(Input::new(&self.scheduler_cron_input))
                        }),
                ),
            )
            .when(self.scheduler.is_some(), |v| {
                v.child(
                    field().label("Run immediately").child(
                        Switch::new("run_immediately")
                            .label("run without next schedule")
                            .small()
                            .checked(self.run_immediately)
                            .on_click(cx.listener(|this, _, _, _| {
                                this.run_immediately = !this.run_immediately
                            })),
                    ),
                )
            })
            .child(
                field()
                    .label("Working directory")
                    .child(Input::new(&self.working_dir)),
            )
            .child(
                field()
                    .label("Environments")
                    .child(Input::new(&self.environments)),
            )
            .into_any_element()
    }

    fn result_view(&mut self, bg_color: Rgba) -> AnyElement {
        let empty_view = div().size_full().bg(bg_color).into_any_element();
        let view = match &self.result {
            CommandResult::Text(Some(x)) => div()
                .p_1()
                .text_sm()
                .size_full()
                .overflow_scrollbar()
                .bg(bg_color)
                .child(x.clone())
                .into_any_element(),
            CommandResult::Text(None) => empty_view,
            CommandResult::Markdown(Some(x)) => TextView::markdown("output", x.clone())
                .bg(bg_color)
                .p_1()
                .size_full()
                .selectable(true)
                .scrollable(true)
                .into_any_element(),
            CommandResult::Markdown(None) => empty_view,
            CommandResult::Html(Some(_)) => match self.result_html_entity.clone() {
                Some(entity) => entity.into_any_element(),
                None => empty_view,
            },
            CommandResult::Html(None) => empty_view,
            CommandResult::Svg(Some(x)) => img(ImageSource::Image(Arc::new(Image::from_bytes(
                ImageFormat::Svg,
                x.clone().into_bytes(),
            ))))
            .bg(bg_color)
            .size_full()
            .object_fit(gpui::ObjectFit::Fill)
            .into_any_element(),
            CommandResult::Svg(None) => empty_view,
        };

        div().relative().size_full().child(view).into_any_element()
    }
}

impl super::Sticker for CommandSticker {
    fn save_on_close(&mut self, cx: &mut Context<Self>) -> bool {
        self.save_config(cx)
    }

    fn min_window_size() -> gpui::Size<i32> {
        gpui::size(100, 100)
    }

    fn default_window_size() -> gpui::Size<i32> {
        gpui::size(300, 400)
    }

    fn set_color(&mut self, color: StickerColor) {
        self.color = color;
    }
}

impl Render for CommandSticker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg_color = Rgba {
            a: 0.85,
            ..self.color.bg()
        };

        window.set_rem_size(px(14.0));

        let mut root = v_flex().relative().size_full();

        let has_result = match &self.result {
            CommandResult::Text(Some(_))
            | CommandResult::Markdown(Some(_))
            | CommandResult::Html(Some(_))
            | CommandResult::Svg(Some(_)) => true,
            CommandResult::Text(None)
            | CommandResult::Markdown(None)
            | CommandResult::Html(None)
            | CommandResult::Svg(None) => false,
        };

        if self.process.is_none() && !has_result && !self.is_schedule_active() {
            root = root
                .bg(bg_color)
                .child(
                    div()
                        .p_2()
                        .h_full()
                        .flex_shrink()
                        .overflow_hidden()
                        .child(v_flex().overflow_y_scrollbar().child(self.form(cx))),
                )
                .child(
                    h_flex().child(
                        Button::new("start")
                            .icon(IconName::Play)
                            .bg(transparent_white())
                            .border_0()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start(window, cx);
                            })),
                    ),
                );
        } else {
            root = root.child(
                div().h_full().flex_shrink().overflow_hidden().child(
                    v_flex()
                        .overflow_y_scrollbar()
                        .child(self.result_view(bg_color)),
                ),
            );

            if self.process.is_some() || self.is_schedule_active() {
                if window.is_window_hovered() && (!self.stopping || self.is_schedule_active()) {
                    root = root.child(
                        h_flex()
                            .bg(bg_color)
                            .items_center()
                            .justify_between()
                            .gap_1()
                            .child(
                                Button::new("stop")
                                    .icon(IconName::Stop)
                                    .when_some(self.next_scheduled_at.clone(), |view, x| {
                                        view.tooltip(format!("Next run at {}", x))
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.stop_schedule();
                                        this.stop(cx);
                                    })),
                            ),
                    );
                }
            } else {
                root = root.child(
                    h_flex()
                        .bg(bg_color)
                        .w_full()
                        .gap_1()
                        .child(
                            Button::new("reset")
                                .icon(IconName::Adjustments)
                                .bg(transparent_white())
                                .border_0()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.result_html_entity = None;
                                    match this.result {
                                        CommandResult::Text(ref mut result)
                                        | CommandResult::Markdown(ref mut result)
                                        | CommandResult::Html(ref mut result)
                                        | CommandResult::Svg(ref mut result) => {
                                            *result = None;
                                        }
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("restart")
                                .icon(IconName::Play)
                                .bg(transparent_white())
                                .border_0()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.start(window, cx);
                                })),
                        ),
                );
            }
        }

        root.when_some(self.error.as_ref(), |view, msg| {
            view.child(Alert::error("error", msg.as_str()).bg(bg_color))
        })
        .when(self.process.is_some(), |view| {
            view.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .bg(yellow_500())
                    .with_animation(
                        "indicator",
                        Animation::new(Duration::from_millis(1000)).repeat(),
                        |v, x| v.opacity(0.1 * x),
                    ),
            )
        })
        .into_any_element()
    }
}

fn kill_process(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        // `Child::kill()` only terminates the direct process. If the child spawns
        // subprocesses that inherit stdout/stderr handles, the pipes can remain
        // open and we keep receiving output. `taskkill /T` kills the whole tree.
        let pid = child.id();
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status();

        if status.is_err() {
            let _ = child.kill();
        }
    }

    #[cfg(not(windows))]
    {
        let _ = child.kill();
    }
}
