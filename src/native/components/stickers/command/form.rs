//! The command sticker's settings form.

use gpui::{AnyElement, Context, prelude::*};
use gpui_component::{
    Sizable,
    button::{Button, ButtonVariants as _},
    form::{field, v_form},
    h_flex,
    input::Input,
    slider::Slider,
    switch::Switch,
    v_flex,
};

use crate::model::content::{CommandResult, Scheduler};

use super::CommandSticker;

/// A button id, its label, and the empty result that selecting it produces.
type ResultMode = (&'static str, &'static str, fn() -> CommandResult);

/// Every way a command's output can be rendered.
const RESULT_MODES: [ResultMode; 5] = [
    ("text", "text", || CommandResult::Text(None)),
    ("markdown", "markdown", || CommandResult::Markdown(None)),
    ("html", "html", || CommandResult::Html(None)),
    ("svg", "svg", || CommandResult::Svg(None)),
    ("source", "file/url", || CommandResult::Source(None)),
];

/// The schedule the "cron" button installs: once a minute.
const DEFAULT_CRON: &str = "0 */1 * * * *";

impl CommandSticker {
    pub(super) fn form(&mut self, cx: &mut Context<Self>) -> AnyElement {
        v_form()
            .child(field().label("Title").child(Input::new(&self.title)))
            .child(field().label("Command").child(Input::new(&self.command)))
            .child(
                field()
                    .label("Render output as")
                    .child(self.result_mode_picker(cx)),
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
                field().label("Selected text").child(
                    Switch::new("accept_selection")
                        .label("Open and run from Ctrl/Cmd + Alt + C")
                        .small()
                        .checked(self.accept_selection)
                        .on_click(cx.listener(|this, _, _, _| {
                            this.accept_selection = !this.accept_selection
                        })),
                ),
            )
            .child(
                field().label("Auto close").child(
                    Switch::new("auto_close")
                        .label("close the sticker after the command succeeded")
                        .small()
                        .checked(self.auto_close)
                        .on_click(cx.listener(|this, _, _, _| this.auto_close = !this.auto_close)),
                ),
            )
            .child(
                field().label("Run without window").child(
                    Switch::new("run_without_window")
                        .label("stay hidden while running, show only when it failed")
                        .small()
                        .checked(self.run_without_window)
                        .on_click(cx.listener(|this, _, _, _| {
                            this.run_without_window = !this.run_without_window
                        })),
                ),
            )
            .child(field().label("Schedule").child(self.schedule_picker(cx)))
            .when(self.scheduler.is_some(), |view| {
                view.child(
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
            .child(
                field()
                    .label(format!("Padding {}", self.padding.read(cx).value().start()))
                    .child(Slider::new(&self.padding)),
            )
            .into_any_element()
    }

    fn result_mode_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut row = h_flex().gap_1().flex_wrap();

        for (id, label, make) in RESULT_MODES {
            let selected = std::mem::discriminant(&self.result) == std::mem::discriminant(&make());
            row = row.child(
                Button::new(id)
                    .label(label)
                    .small()
                    .when(selected, |view| view.primary())
                    .occlude()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Switching the render mode drops the old payload: it was produced for a
                        // different renderer and would not display correctly.
                        this.result = make();
                        this.result_html_entity = None;
                        this.result_file_entity = None;
                        cx.notify();
                    })),
            );
        }

        row.into_any_element()
    }

    fn schedule_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let is_cron = matches!(self.scheduler, Some(Scheduler::Cron(_)));

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
                            .when(self.scheduler.is_none(), |view| view.primary())
                            .occlude()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.scheduler = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("cron")
                            .label("cron")
                            .small()
                            .when(is_cron, |view| view.primary())
                            .occlude()
                            .on_click(cx.listener(|this, _, window, cx| {
                                // Keep whatever expression the user already typed; only fall back
                                // to the default when the input is empty. Setting the scheduler
                                // here rather than relying on the input's change event means
                                // re-selecting the same expression still arms the schedule.
                                let current = this
                                    .scheduler_cron_input
                                    .read(cx)
                                    .value()
                                    .trim()
                                    .to_string();
                                let expr = if current.is_empty() {
                                    DEFAULT_CRON.to_string()
                                } else {
                                    current
                                };

                                this.scheduler = Some(Scheduler::Cron(expr.clone()));
                                this.scheduler_cron_input
                                    .update(cx, |input, cx| input.set_value(expr, window, cx));
                                cx.notify();
                            })),
                    ),
            )
            .when(is_cron, |view| {
                view.child(Input::new(&self.scheduler_cron_input))
            })
            .into_any_element()
    }
}
