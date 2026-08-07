//! Rendering a command sticker: its result, its footer controls and its running overlay.

use std::{sync::Arc, time::Duration};

use gpui::{
    Animation, AnimationExt, AnyElement, Context, Entity, Image, ImageFormat, ImageSource, Render,
    Rgba, Window, div, img, prelude::*, px, transparent_white,
};
use gpui_component::{
    alert::Alert, button::Button, h_flex, scroll::ScrollableElement, text::TextView, v_flex,
    yellow_500,
};

use crate::{
    model::{
        content::{CommandResult, FileStickerContent},
        sticker::StickerColor,
    },
    native::{
        components::{IconName, stickers::file::FileSticker},
        windows::StickerWindowEvent,
    },
    storage::ArcStickerStore,
};

use super::CommandSticker;

/// How opaque the "a command is running" wash gets at the peak of its pulse.
const RUNNING_OVERLAY_PEAK_OPACITY: f32 = 0.1;
const RUNNING_OVERLAY_PERIOD: Duration = Duration::from_millis(1000);

/// Wrap a file path or URL produced by a command in a nested file sticker.
pub(super) fn build_file_content(
    id: i64,
    source: &str,
    color: StickerColor,
    store: ArcStickerStore,
    window: &mut Window,
    cx: &mut Context<CommandSticker>,
    sticker_events_tx: std::sync::mpsc::Sender<StickerWindowEvent>,
) -> Entity<FileSticker> {
    // Commands emit the path on stdout, so it arrives with the trailing newline still attached.
    let sources = vec![source.replace('\n', "").trim().to_string()];
    let content = FileStickerContent::from_sources(&sources).to_json();
    cx.new(move |cx| FileSticker::new(id, color, store, &content, window, cx, sticker_events_tx))
}

impl CommandSticker {
    /// Show the settings instead of the output when there is nothing to show yet.
    pub(super) fn show_editing_view(&self) -> bool {
        if self.open_in_settings && self.process.is_none() {
            return true;
        }

        self.process.is_none() && self.result.value().is_none() && !self.is_schedule_active()
    }

    pub(super) fn result_view(&mut self, bg_color: Rgba, cx: &Context<Self>) -> AnyElement {
        let padding = px(self.padding.read(cx).value().start());
        let empty_view = || div().size_full().bg(bg_color).into_any_element();

        let view = match &self.result {
            CommandResult::Text(Some(text)) => div()
                .p(padding)
                .text_sm()
                .size_full()
                .overflow_scrollbar()
                .bg(bg_color)
                .child(text.clone())
                .into_any_element(),
            CommandResult::Markdown(Some(text)) => TextView::markdown("output", text.clone())
                .bg(bg_color)
                .p(padding)
                .size_full()
                .selectable(true)
                .scrollable(true)
                .into_any_element(),
            CommandResult::Svg(Some(svg)) => img(ImageSource::Image(Arc::new(Image::from_bytes(
                ImageFormat::Svg,
                svg.clone().into_bytes(),
            ))))
            .bg(bg_color)
            .p(padding)
            .size_full()
            .object_fit(gpui::ObjectFit::Cover)
            .into_any_element(),
            CommandResult::Html(Some(_)) => match self.result_html_entity.clone() {
                Some(entity) => div()
                    .size_full()
                    .child(entity)
                    .p(padding)
                    .into_any_element(),
                None => empty_view(),
            },
            CommandResult::Source(Some(_)) => match self.result_file_entity.clone() {
                Some(entity) => div().size_full().child(entity).into_any_element(),
                None => empty_view(),
            },
            _ => empty_view(),
        };

        div().relative().size_full().child(view).into_any_element()
    }

    /// The controls in the sticker's footer, which follow whatever the command is doing.
    pub(super) fn footer(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.show_editing_view() {
            return Some(
                Button::new("start")
                    .icon(IconName::Play)
                    .bg(transparent_white())
                    .border_0()
                    .occlude()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.start(window, cx, false);
                    }))
                    .into_any_element(),
            );
        }

        if self.process.is_some() || self.is_schedule_active() {
            // While stopping there is nothing left to press, unless a schedule is still armed.
            return (!self.stopping || self.is_schedule_active()).then(|| {
                Button::new("stop")
                    .icon(IconName::Stop)
                    .when_some(self.next_scheduled_at.clone(), |view, next_run| {
                        view.tooltip(format!("Next run at {next_run}"))
                    })
                    .bg(transparent_white())
                    .border_0()
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.stop_schedule();
                        this.stop(cx);
                    }))
                    .into_any_element()
            });
        }

        Some(
            h_flex()
                .gap_1()
                .child(
                    Button::new("reset")
                        .icon(IconName::Adjustments)
                        .bg(transparent_white())
                        .border_0()
                        .occlude()
                        .on_click(cx.listener(|this, _, _, cx| {
                            // Dropping the result is what sends the sticker back to its settings.
                            this.result.clear();
                            this.result_html_entity = None;
                            this.result_file_entity = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("restart")
                        .icon(IconName::Play)
                        .bg(transparent_white())
                        .border_0()
                        .occlude()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.start(window, cx, false);
                        })),
                )
                .into_any_element(),
        )
    }
}

impl Render for CommandSticker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg_color = Rgba {
            a: 0.85,
            ..self.color.bg()
        };

        window.set_rem_size(px(14.0));

        let body = if self.show_editing_view() {
            div()
                .p_2()
                .size_full()
                .overflow_scrollbar()
                .child(self.form(cx))
                .into_any_element()
        } else {
            div()
                .h_full()
                .overflow_hidden()
                .child(
                    v_flex()
                        .overflow_y_scrollbar()
                        .child(self.result_view(bg_color, cx)),
                )
                .into_any_element()
        };

        v_flex()
            .relative()
            .size_full()
            .child(body)
            .when_some(self.error.as_ref(), |view, message| {
                view.child(Alert::error("error", message.as_str()).bg(bg_color))
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
                            Animation::new(RUNNING_OVERLAY_PERIOD).repeat(),
                            |view, delta| view.opacity(RUNNING_OVERLAY_PEAK_OPACITY * delta),
                        ),
                )
            })
            .into_any_element()
    }
}
