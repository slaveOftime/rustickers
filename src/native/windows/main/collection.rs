//! The collection projection and its virtualized row presentation.
use super::*;
use crate::model::content::{CommandContent, Scheduler};

#[derive(Clone, Debug)]
pub(super) struct CommandSummary {
    pub(super) command: String,
    pub(super) working_dir: String,
    pub(super) cron: Option<String>,
    pub(super) armed: bool,
    pub(super) accept_selection: bool,
}
impl CommandSummary {
    pub(super) fn parse(content: &str) -> Option<Self> {
        let content: CommandContent = serde_json::from_str(content).ok()?;
        let cron = content.scheduler.map(|Scheduler::Cron(cron)| cron);
        Some(Self {
            command: content.command,
            working_dir: content.working_dir,
            armed: content.started_at.is_some()
                && cron.as_ref().is_some_and(|c| !c.trim().is_empty()),
            cron,
            accept_selection: content.accept_selection,
        })
    }
    fn subtitle(&self) -> String {
        let command = self.command.replace(['\r', '\n'], " ");
        if self.working_dir.is_empty() {
            command
        } else {
            format!("{command}  ·  {}", self.working_dir)
        }
    }
}

impl MainWindow {
    pub(super) fn rebuild_visible(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected_sticker_id();
        let query = self.query.read(cx).value().to_lowercase();
        self.visible = self
            .stickers
            .iter()
            .enumerate()
            .filter_map(|(ix, sticker)| {
                let command = self.commands.get(&sticker.id);
                let matches_query = query.is_empty()
                    || sticker.title.to_lowercase().contains(&query)
                    || command.is_some_and(|c| {
                        c.command.to_lowercase().contains(&query)
                            || c.working_dir.to_lowercase().contains(&query)
                            || c.cron
                                .as_ref()
                                .is_some_and(|cron| cron.to_lowercase().contains(&query))
                    });
                (matches_query && self.section.matches(sticker, command)).then_some(ix)
            })
            .collect();
        self.selected_index = selected.and_then(|id| {
            self.visible
                .iter()
                .position(|&ix| self.stickers[ix].id == id)
        });
        cx.notify();
    }
    pub(super) fn collection(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let count = self.visible.len();
        let view = cx.entity();
        div()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .track_focus(&self.list_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_list_key(event, window, cx)
            }))
            .when(count > 0, |v| {
                v.child(
                    uniform_list("manager-stickers", count, move |range, _, cx| {
                        view.update(cx, |this, cx| {
                            range
                                .filter_map(|ix| {
                                    this.visible
                                        .get(ix)
                                        .and_then(|&source| this.stickers.get(source))
                                        .map(|s| this.sticker_card(ix, s, cx))
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .size_full()
                    .track_scroll(&self.list_scroll),
                )
            })
            .when(count == 0, |v| {
                v.child(
                    v_flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .p_6()
                        .text_color(cx.theme().muted_foreground)
                        .child(Icon::new(self.section.icon()).large())
                        .child(
                            div()
                                .text_base()
                                .font_medium()
                                .text_color(cx.theme().foreground)
                                .child(if self.loading {
                                    "Loading stickers…"
                                } else if self.error.is_some() {
                                    "Could not load stickers"
                                } else if !self.query.read(cx).value().is_empty() {
                                    "No matching stickers"
                                } else {
                                    "A place for your next idea"
                                }),
                        )
                        .child(
                            div()
                                .text_sm()
                                .child(if !self.query.read(cx).value().is_empty() {
                                    "Try another title, command, or folder."
                                } else if self.section == Section::Scheduled {
                                    "Add a cron schedule and start a command to arm it."
                                } else if self.section == Section::Selection {
                                    "Enable selection input in a command’s settings."
                                } else {
                                    "Create a sticker to keep something useful close."
                                }),
                        )
                        .child(self.create_button(cx)),
                )
            })
            .into_any_element()
    }
    fn sticker_card(
        &self,
        ix: usize,
        sticker: &StickerBrief,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = sticker.id;
        let title = if sticker.title.is_empty() {
            "Untitled sticker".to_string()
        } else {
            sticker.title.clone()
        };
        let selected = self.selected_index == Some(ix);
        let command = self.commands.get(&id);
        let running = command_activity::is_running(id);
        let activity = CardActivity::of_command(running, command_activity::next_run(id));
        let status = activity.status_line(if sticker.state == StickerState::Open {
            "On desktop"
        } else {
            "Closed"
        });
        let subtitle = command.map(CommandSummary::subtitle).unwrap_or_else(|| {
            format!(
                "{}  ·  {}",
                type_label(sticker.sticker_type),
                crate::utils::time::format_unix_millis(sticker.updated_at)
            )
        });
        let delete_title = title.clone();
        h_flex()
            .w_full()
            .id(("sticker-row", id as usize))
            .group("manager-row")
            .h(gpui::rems(ROW_HEIGHT))
            .px_5()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(if selected {
                cx.theme().list_active
            } else {
                cx.theme().background
            })
            .when(!selected, |v| v.hover(|v| v.bg(cx.theme().list_hover)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.selected_index = Some(ix);
                    this.list_focus.focus(window, cx);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                    if event.click_count >= 2 {
                        this.open_sticker(id, cx);
                    }
                }),
            )
            .child(
                div()
                    .size_8()
                    .flex_shrink_0()
                    .rounded_md()
                    .bg(cx.theme().muted)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(if sticker.color == StickerColor::Gray {
                        cx.theme().muted_foreground
                    } else {
                        sticker.color.swatch().into()
                    })
                    .child(Icon::new(sticker_type_icon(&sticker.sticker_type)).small()),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(div().text_sm().font_medium().truncate().child(title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(subtitle),
                    )
                    .when_some(command, |v, c| {
                        v.child(
                            h_flex()
                                .min_w_0()
                                .overflow_hidden()
                                .gap_2()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .when_some(c.cron.as_ref(), |v, cron| {
                                    v.child(div().flex_1().min_w_0().truncate().child(format!(
                                        "{} · {cron}",
                                        if c.armed {
                                            "Scheduled"
                                        } else {
                                            "Schedule not armed"
                                        }
                                    )))
                                })
                                .when(c.accept_selection, |v| {
                                    v.child(div().flex_shrink_0().child("Selection input"))
                                }),
                        )
                    }),
            )
            .child(
                v_flex()
                    .items_end()
                    .gap_1()
                    .flex_shrink_0()
                    .w(gpui::rems(12.))
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .justify_end()
                            .gap_1()
                            .text_xs()
                            .text_color(if running {
                                cx.theme().warning
                            } else {
                                cx.theme().muted_foreground
                            })
                            .when(running, |v| v.child(Spinner::new().small()))
                            .child(div().min_w_0().truncate().child(status)),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .invisible()
                            .group_hover("manager-row", |v| v.visible())
                            .when(selected, |v| v.visible())
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                Button::new(("open-sticker", id as u64))
                                    .small()
                                    .ghost()
                                    .icon(IconName::ArrowRight)
                                    .accessibility_label("Open sticker")
                                    .tooltip("Open sticker (Enter)")
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.open_sticker(id, cx)
                                        }),
                                    ),
                            )
                            .child(
                                Button::new(("delete-sticker", id as u64))
                                    .small()
                                    .ghost()
                                    .icon(IconName::Delete)
                                    .accessibility_label("Delete sticker")
                                    .tooltip("Delete sticker (Delete)")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.delete_sticker(id, delete_title.clone(), window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
fn type_label(kind: StickerType) -> &'static str {
    match kind {
        StickerType::Markdown => "Text",
        StickerType::Command => "Command",
        StickerType::Timer => "Timer",
        StickerType::Paint => "Sketch",
        StickerType::File => "File",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_content_never_claims_automation_state() {
        assert!(CommandSummary::parse("not json").is_none());
    }
    #[test]
    fn a_schedule_needs_an_arming_timestamp_and_expression() {
        let mut c = CommandContent {
            command: "echo hi".into(),
            scheduler: Some(Scheduler::Cron("0 * * * * *".into())),
            ..Default::default()
        };
        assert!(
            !CommandSummary::parse(&serde_json::to_string(&c).unwrap())
                .unwrap()
                .armed
        );
        c.started_at = Some(123);
        assert!(
            CommandSummary::parse(&serde_json::to_string(&c).unwrap())
                .unwrap()
                .armed
        );
        c.scheduler = Some(Scheduler::Cron("  ".into()));
        assert!(
            !CommandSummary::parse(&serde_json::to_string(&c).unwrap())
                .unwrap()
                .armed
        );
    }
}
