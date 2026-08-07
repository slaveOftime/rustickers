//! The command sticker: runs a program and renders its output.
//!
//! The pieces are split by concern so the headless half can be reused:
//!
//! - [`runner`] spawns the process and reads its pipes. No GPUI.
//! - [`schedule`] parses cron expressions and works out the next fire time. No GPUI.
//! - [`activity`] is the process-wide record of what every command sticker is doing.
//! - [`background`] runs armed schedules while their windows are closed.
//! - [`execution`], [`form`] and [`output`] are the window's behaviour, settings UI and result UI.

pub mod activity;
pub mod background;
mod execution;
mod form;
mod output;
pub mod runner;
pub mod schedule;

use std::sync::{Arc, Mutex};

use gpui::{Context, Entity, Window, prelude::*};
use gpui_component::{
    input::{InputEvent, InputState},
    slider::SliderState,
};

use crate::{
    model::{
        content::{CommandContent, CommandResult, Scheduler},
        sticker::StickerColor,
    },
    native::{
        components::{stickers::file::FileSticker, webview::SimpleWebView},
        windows::StickerWindowEvent,
    },
    storage::ArcStickerStore,
};

pub struct CommandSticker {
    id: i64,
    color: StickerColor,
    store: ArcStickerStore,
    sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,

    title: Entity<InputState>,
    command: Entity<InputState>,
    environments: Entity<InputState>,
    working_dir: Entity<InputState>,
    scheduler: Option<Scheduler>,
    scheduler_cron_input: Entity<InputState>,
    run_immediately: bool,
    stream_result: bool,
    padding: Entity<SliderState>,
    started_at: Option<i64>,

    result: CommandResult,
    result_html_entity: Option<Entity<SimpleWebView>>,
    result_file_entity: Option<Entity<FileSticker>>,

    process: Option<Arc<Mutex<std::process::Child>>>,
    /// Held for as long as `process` is, so the sticker list can show a running indicator.
    run_guard: Option<activity::RunGuard>,
    stopping: bool,

    schedule_cancel: Option<schedule::CancelToken>,
    next_scheduled_at: Option<String>,
    error: Option<String>,
    accept_selection: bool,
    auto_close: bool,
    run_without_window: bool,
    /// Opened from the sticker list while `auto_close`/`run_without_window` is on: show the
    /// settings view instead of restoring the previous run.
    open_in_settings: bool,
    /// The host window was created hidden, it is only revealed when the command fails.
    window_hidden: bool,
    selection: Option<String>,
    /// Tells the background scheduler to stand down: while this window lives it drives the
    /// schedule itself. Dropped with the sticker, which is what hands the schedule back.
    _window_claim: activity::WindowClaim,
}

/// What the sticker asks its host window to do.
pub enum CommandStickerWindowRequest {
    Close,
    Show,
}

impl gpui::EventEmitter<CommandStickerWindowRequest> for CommandSticker {}

/// Everything a command sticker needs to know about itself when its window opens.
pub struct CommandStickerInit {
    pub id: i64,
    pub color: StickerColor,
    pub store: ArcStickerStore,
    pub title: String,
    pub content: String,
    pub sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,
    /// The text the user had selected when the sticker was launched from the selection hotkey.
    pub selection: Option<String>,
    /// Start in the settings view instead of running or restoring the previous result.
    pub open_in_settings: bool,
    /// The host window was created hidden and only shows itself when the sticker asks for it.
    pub window_hidden: bool,
}

impl CommandSticker {
    pub fn new(init: CommandStickerInit, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let CommandStickerInit {
            id,
            color,
            store,
            title,
            content,
            sticker_events_tx,
            selection,
            open_in_settings,
            window_hidden,
        } = init;

        let cmd = serde_json::from_str::<CommandContent>(&content).unwrap_or_default();
        let open_in_settings = open_in_settings && (cmd.auto_close || cmd.run_without_window);

        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(title)
                .placeholder("Optional title")
        });

        let command = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(cmd.command)
                .multi_line(true)
                .auto_grow(1, 5)
                .placeholder("command with args")
        });

        let environments = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .auto_grow(1, 5)
                .default_value(cmd.environments)
                .placeholder("KEY=VALUE per line")
        });

        let working_dir = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(cmd.working_dir)
                .placeholder("Optional")
        });

        let cron = match &cmd.scheduler {
            Some(Scheduler::Cron(cron)) => cron.clone(),
            None => String::new(),
        };
        let cron_entity = cx.new(|cx| InputState::new(window, cx).default_value(cron));

        let result_html_entity = match &cmd.result {
            CommandResult::Html(Some(html)) if !open_in_settings => {
                Some(cx.new(|cx| SimpleWebView::new(html.as_str(), window, cx)))
            }
            _ => None,
        };

        let result_file_entity = match &cmd.result {
            CommandResult::Source(Some(source)) if !open_in_settings => {
                Some(output::build_file_content(
                    id,
                    source,
                    color,
                    store.clone(),
                    window,
                    cx,
                    sticker_events_tx.clone(),
                ))
            }
            _ => None,
        };

        let padding = cx.new(|_cx| {
            SliderState::new()
                .default_value(cmd.padding.unwrap_or(0) as f32)
                .min(0.0)
                .max(64.0)
                .step(1.0)
        });

        cx.subscribe(&cron_entity, |this, input, event, cx| {
            if let InputEvent::Change = event {
                this.scheduler = Some(Scheduler::Cron(input.read(cx).value().trim().to_string()));
            }
        })
        .detach();

        let root_entity = cx.entity();
        window
            .spawn(cx, async move |cx| {
                let _ = cx.update_window_entity(&root_entity, |this, window, cx| {
                    this.on_opened(window, cx);
                });
            })
            .detach();

        Self {
            id,
            color,
            store,
            sticker_events_tx,

            title,
            command,
            environments,
            working_dir,
            scheduler: cmd.scheduler,
            scheduler_cron_input: cron_entity,
            run_immediately: cmd.run_immediately,

            result: cmd.result,
            result_html_entity,
            result_file_entity,

            stream_result: cmd.stream_result,
            padding,
            started_at: cmd.started_at,
            accept_selection: cmd.accept_selection,
            auto_close: cmd.auto_close,
            run_without_window: cmd.run_without_window,
            open_in_settings,
            window_hidden,
            selection,
            _window_claim: activity::claim_window(id),

            process: None,
            run_guard: None,
            stopping: false,

            schedule_cancel: None,
            next_scheduled_at: None,
            error: None,
        }
    }

    /// Decide what the sticker does the moment its window is ready.
    fn on_opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_in_settings {
            // Opened to be edited: don't auto run and don't restore the last result.
        } else if self.selection.is_some() {
            self.started_at = Some(crate::utils::time::now_unix_millis());
            self.run(window, cx);
        } else if self.started_at.is_some() && self.process.is_none() && !self.is_schedule_active()
        {
            self.start(window, cx, true);
        } else {
            // Nothing is going to run, so never leave an invisible window behind.
            self.reveal_window(cx);
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
            padding: Some(self.padding.read(cx).value().start() as u8),
            started_at: self.started_at,
            accept_selection: self.accept_selection,
            auto_close: self.auto_close,
            run_without_window: self.run_without_window,
        }
    }

    fn save_config(&mut self, cx: &mut Context<Self>) -> bool {
        let content = self.build_content(cx);
        let title = self.title.read(cx).value().trim().to_string();
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

    /// Report a failure, and reveal the window when the command is running hidden.
    fn fail(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        self.reveal_window(cx);
        cx.notify();
    }

    fn reveal_window(&mut self, cx: &mut Context<Self>) {
        if self.window_hidden {
            self.window_hidden = false;
            cx.emit(CommandStickerWindowRequest::Show);
        }
    }
}

impl super::Sticker for CommandSticker {
    fn id(&self) -> i64 {
        self.id
    }

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

    fn use_default_bg(&self) -> bool {
        self.show_editing_view()
    }

    fn disable_color_picker(&self) -> bool {
        !self.show_editing_view()
    }

    fn footer_extension(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        self.footer(cx)
    }

    fn is_footer_absoute(&self) -> bool {
        match self.result {
            CommandResult::Html(_) => self.show_editing_view(),
            _ => true,
        }
    }
}
