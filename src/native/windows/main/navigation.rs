//! Manager destinations and the small, shared filter policy.
use super::*;
use crate::gpui_component::sidebar::{
    Sidebar, SidebarGroup, SidebarItem, SidebarMenu, SidebarMenuItem,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Section {
    #[default]
    Text,
    Commands,
    Timers,
    Sketches,
    Files,
    Scheduled,
    Selection,
    Settings,
}
impl Section {
    pub(super) const COLLECTIONS: [Self; 5] = [
        Self::Text,
        Self::Commands,
        Self::Timers,
        Self::Sketches,
        Self::Files,
    ];
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Commands => "Commands",
            Self::Timers => "Timers",
            Self::Sketches => "Sketches",
            Self::Files => "Files",
            Self::Scheduled => "Scheduled",
            Self::Selection => "Selection commands",
            Self::Settings => "Settings",
        }
    }
    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Text => "Notes and protected text. Clipboard history lives in TXTs.",
            Self::Commands => "Commands, output, and the routines you rely on.",
            Self::Timers => "A little structure for your time.",
            Self::Sketches => "A space to think in strokes.",
            Self::Files => "Local files, without changing the source.",
            Self::Scheduled => "Armed routines keep running with their windows closed.",
            Self::Selection => "Send selected text to these commands with your selection shortcut.",
            Self::Settings => "Privacy, collection, and the shortcuts that keep you fast.",
        }
    }
    pub(super) fn kind(self) -> Option<StickerType> {
        match self {
            Self::Text => Some(StickerType::Markdown),
            Self::Commands | Self::Scheduled | Self::Selection => Some(StickerType::Command),
            Self::Timers => Some(StickerType::Timer),
            Self::Sketches => Some(StickerType::Paint),
            Self::Files => Some(StickerType::File),
            _ => None,
        }
    }
    pub(super) fn icon(self) -> IconName {
        match self {
            Self::Scheduled => IconName::Loop,
            Self::Selection => IconName::Forward,
            Self::Settings => IconName::Adjustments,
            _ => sticker_type_icon(&self.kind().unwrap_or(StickerType::Markdown)),
        }
    }
    pub(super) fn matches(self, sticker: &StickerBrief, command: Option<&CommandSummary>) -> bool {
        match self {
            Self::Settings => false,
            Self::Scheduled => command.is_some_and(|c| c.armed),
            Self::Selection => command.is_some_and(|c| c.accept_selection),
            _ => self.kind().is_none_or(|kind| kind == sticker.sticker_type),
        }
    }
}

impl MainWindow {
    pub(super) fn select_section(
        &mut self,
        section: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.section = section;
        self.query
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.selected_index = None;
        self.rebuild_visible(cx);
        self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
        if section == Section::Settings {
            self.load_settings(cx);
        }
        cx.notify();
    }
    pub(super) fn navigation(
        &self,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let item =
            |section: Section, cx: &mut Context<Self>| {
                let count = self
                    .stickers
                    .iter()
                    .filter(|s| section.matches(s, self.commands.get(&s.id)))
                    .count();
                SidebarMenuItem::new(section.label())
                    .icon(section.icon())
                    .active(self.section == section)
                    .suffix(move |_, cx| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(count.to_string())
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_section(section, window, cx)
                    }))
            };
        Sidebar::new("manager-navigation")
            .w(gpui::rems(13.))
            .collapsed(compact)
            .header(
                h_flex()
                    .h_10()
                    .pl_1()
                    .gap_2()
                    .child(gpui::img("icon.png").size_6().flex_shrink_0())
                    .when(!compact, |v| {
                        v.child(div().font_semibold().child("Rustickers"))
                    }),
            )
            .child(SidebarGroup::new("Library").child(
                SidebarMenu::new().children(Section::COLLECTIONS.into_iter().map(|s| item(s, cx))),
            ))
            .child(
                SidebarGroup::new("Automation").child(
                    SidebarMenu::new()
                        .child(item(Section::Scheduled, cx))
                        .child(item(Section::Selection, cx)),
                ),
            )
            .footer(
                v_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        SidebarMenu::new()
                            .collapsed(compact)
                            .child(
                                SidebarMenuItem::new("Open TXT")
                                    .icon(IconName::Copy)
                                    .collapsed(compact)
                                    .active(false)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Err(error) = super::super::text::open_from_main(
                                            cx,
                                            this.sticker_events_sender.clone(),
                                        ) {
                                            this.error =
                                                Some(format!("Could not open TXTs: {error}"));
                                            cx.notify();
                                        }
                                    })),
                            )
                            .render("manager-open-txt-menu", window, cx),
                    )
                    .child(
                        SidebarMenu::new()
                            .collapsed(compact)
                            .child(
                                SidebarMenuItem::new("Settings")
                                    .icon(IconName::Adjustments)
                                    .collapsed(compact)
                                    .active(self.section == Section::Settings)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.select_section(Section::Settings, window, cx)
                                    })),
                            )
                            .render("manager-settings-menu", window, cx),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sticker(kind: StickerType) -> StickerBrief {
        StickerBrief {
            id: 1,
            title: "Sample".into(),
            state: StickerState::Close,
            color: StickerColor::Gray,
            sticker_type: kind,
            created_at: 0,
            updated_at: 0,
        }
    }
    #[test]
    fn library_filters_are_type_specific_and_include_closed_stickers() {
        for section in Section::COLLECTIONS {
            for kind in [
                StickerType::Markdown,
                StickerType::Command,
                StickerType::Timer,
                StickerType::Paint,
                StickerType::File,
            ] {
                assert_eq!(
                    section.matches(&sticker(kind), None),
                    section.kind().is_none_or(|k| k == kind)
                );
            }
        }
    }
    #[test]
    fn automation_filters_require_the_actual_command_configuration() {
        let c = CommandSummary {
            command: "echo hello".into(),
            working_dir: String::new(),
            cron: Some("0 * * * * *".into()),
            armed: false,
            accept_selection: true,
        };
        assert!(!Section::Scheduled.matches(&sticker(StickerType::Command), Some(&c)));
        assert!(Section::Selection.matches(&sticker(StickerType::Command), Some(&c)));
        assert!(!Section::Scheduled.matches(&sticker(StickerType::Command), None));
        assert!(!Section::Settings.matches(&sticker(StickerType::Markdown), None));
    }
}
