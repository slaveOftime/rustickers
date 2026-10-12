//! The manager owns collection navigation, refresh, and sticker lifecycle commands.
mod collection;
mod navigation;
mod settings;

use crate::{
    gpui_component::{
        Root,
        alert::Alert,
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState},
        menu::{DropdownMenu, PopupMenuItem},
        spinner::Spinner,
        *,
    },
    model::sticker::*,
    native::{
        components::{
            IconName,
            stickers::{
                Sticker,
                command::{CommandSticker, activity as command_activity},
                file::FileSticker,
                markdown::MarkdownSticker,
                paint::PaintSticker,
                timer::TimerSticker,
            },
        },
        windows::{StickerWindowEvent, sticker::StickerWindow, text},
    },
    storage::ArcStickerStore,
};
use collection::CommandSummary;
use gpui::{
    AnyWindowHandle, App, AsyncApp, Bounds, Context, Entity, FocusHandle, Focusable, IntoElement,
    KeyDownEvent, MouseButton, MouseUpEvent, Render, ScrollStrategy, UniformListScrollHandle,
    WeakEntity, Window, WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowOptions,
    div, prelude::*, px, size, uniform_list,
};
use navigation::Section;
use std::{collections::HashMap, sync::mpsc, time::Duration};

const STICKER_LOAD_LIMIT: i64 = 10000;
const STICKER_EVENT_PUMP_INTERVAL: Duration = Duration::from_millis(120);
const PAGE_STEP: usize = 6;

pub struct MainWindow {
    store: ArcStickerStore,
    sticker_events_sender: mpsc::Sender<StickerWindowEvent>,
    query: Entity<InputState>,
    order: StickerOrderBy,
    stickers: Vec<StickerBrief>,
    commands: HashMap<i64, CommandSummary>,
    /// Source coordinates; selection and scrolling use coordinates within this projection.
    visible: Vec<usize>,
    section: Section,
    selected_index: Option<usize>,
    list_focus: FocusHandle,
    list_scroll: UniformListScrollHandle,
    loading: bool,
    error: Option<String>,
    load_request_token: u64,
    creating: bool,
    clipboard_settings: Option<crate::clipboard::settings::ClipboardSettings>,
    password_configured: Option<bool>,
    settings_busy: bool,
    settings_revision: u64,
    shortcuts: [Entity<InputState>; 4],
    shortcuts_dirty: bool,
    shortcuts_busy: bool,
    shortcuts_error: Option<String>,
}

impl MainWindow {
    pub fn open(
        cx: &mut App,
        rx: mpsc::Receiver<StickerWindowEvent>,
        tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
    ) -> anyhow::Result<AnyWindowHandle> {
        // WindowOptions takes logical pixels; the platform handles display scaling.
        let bounds = Bounds::centered(None, size(px(820.), px(620.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(560.), px(480.))),
                window_background: WindowBackgroundAppearance::Transparent,
                titlebar: None,
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| Self::new(window, cx, rx, tx, store));
                // Reapply logical size: borderless Windows creation can initially
                // size in physical pixels while GPUI lays out logical pixels.
                #[cfg(target_os = "windows")]
                window.resize(bounds.size);
                cx.new(|cx| Root::new(view, window, cx).bg(gpui::transparent_black()))
            },
        )
        .map(Into::into)
    }
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        rx: mpsc::Receiver<StickerWindowEvent>,
        tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search stickers…"));
        let bindings = crate::native::hotkey::bindings();
        let shortcuts = std::array::from_fn(|ix| {
            cx.new(|cx| InputState::new(window, cx).default_value(bindings[ix].clone()))
        });
        for input in &shortcuts {
            cx.subscribe(input, |this, _, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    this.shortcuts_dirty = true;
                    this.shortcuts_error = None;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.save_shortcuts(cx),
                _ => {}
            })
            .detach();
        }
        window.on_window_should_close(cx, Self::request_quit);
        cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.rebuild_visible(cx);
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |this, cx| this.spawn_load_stickers(cx));
            Self::loop_events(this, rx, cx).await;
        })
        .detach();
        // Land the cursor in the search box on open. Mirrors the TXTs picker so
        // every manager launch is one keystroke away from filtering stickers.
        query.update(cx, |input, cx| input.focus(window, cx));
        Self {
            store,
            sticker_events_sender: tx,
            query,
            order: StickerOrderBy::UpdatedDesc,
            stickers: Vec::new(),
            commands: HashMap::new(),
            visible: Vec::new(),
            section: Section::Text,
            selected_index: None,
            list_focus: cx.focus_handle().tab_index(1).tab_stop(true),
            list_scroll: UniformListScrollHandle::default(),
            loading: false,
            error: None,
            load_request_token: 0,
            creating: false,
            clipboard_settings: None,
            password_configured: None,
            settings_busy: false,
            settings_revision: 0,
            shortcuts,
            shortcuts_dirty: false,
            shortcuts_busy: false,
            shortcuts_error: None,
        }
    }
    fn request_quit(window: &mut Window, cx: &mut App) -> bool {
        if text::password::is_rotating() || text::password::has_dirty_editors() {
            window.open_alert_dialog(cx, |dialog, _, _| dialog.title("Finish encrypted text edits").child("Save or discard encrypted drafts and wait for password changes before quitting."));
            false
        } else {
            text::clear_owned_secret();
            cx.quit();
            true
        }
    }
    async fn loop_events(
        this: WeakEntity<Self>,
        rx: mpsc::Receiver<StickerWindowEvent>,
        cx: &mut AsyncApp,
    ) {
        let mut activity = command_activity::generation();
        let mut text_version = -1;
        loop {
            cx.background_executor()
                .timer(STICKER_EVENT_PUMP_INTERVAL)
                .await;
            let events: Vec<_> = rx.try_iter().collect();
            let generation = command_activity::generation();
            let activity_changed = generation != activity;
            activity = generation;
            let version = if let Some(repo) = text::text_repository() {
                repo.data_version().await.unwrap_or(text_version)
            } else {
                text_version
            };
            let text_changed = version != text_version;
            text_version = version;
            if events.is_empty() && !activity_changed && !text_changed {
                if this.update(cx, |_, _| {}).is_err() {
                    break;
                }
                continue;
            }
            if this
                .update(cx, |this, cx| {
                    let reload = text_changed
                        || events
                            .iter()
                            .any(|e| matches!(e, StickerWindowEvent::Created { .. }));
                    for event in events {
                        match event {
                            StickerWindowEvent::TitleChanged { id, title } => {
                                if let Some(s) = this.stickers.iter_mut().find(|s| s.id == id) {
                                    s.title = title;
                                }
                            }
                            StickerWindowEvent::ColorChanged { id, color } => {
                                if let Some(s) = this.stickers.iter_mut().find(|s| s.id == id) {
                                    s.color = color;
                                }
                            }
                            StickerWindowEvent::Closed { id } => {
                                if let Some(s) = this.stickers.iter_mut().find(|s| s.id == id) {
                                    s.state = StickerState::Close;
                                }
                            }
                            StickerWindowEvent::Created { .. } => {}
                        }
                    }
                    if reload {
                        this.spawn_load_stickers(cx);
                        if this.section == Section::Settings && !this.settings_busy {
                            this.load_settings(cx);
                        }
                    } else {
                        this.rebuild_visible(cx);
                    }
                })
                .is_err()
            {
                break;
            }
        }
    }
    fn spawn_load_stickers(&mut self, cx: &mut Context<Self>) {
        self.load_request_token = self.load_request_token.wrapping_add(1);
        let token = self.load_request_token;
        let store = self.store.clone();
        let order = self.order;
        self.loading = true;
        cx.notify();
        cx.spawn(async move |entity, cx| {
            let result = async {
                let stickers = store
                    .query_stickers(None, order, STICKER_LOAD_LIMIT, 0)
                    .await?;
                let commands = store.get_command_stickers().await?;
                anyhow::Ok((stickers, commands))
            }
            .await;
            let _ = entity.update(cx, |this, cx| {
                if token != this.load_request_token {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((stickers, commands)) => {
                        let selected = this.selected_sticker_id();
                        this.stickers = stickers;
                        this.commands = commands
                            .into_iter()
                            .filter_map(|s| CommandSummary::parse(&s.content).map(|c| (s.id, c)))
                            .collect();
                        this.rebuild_visible(cx);
                        this.selected_index = selected.and_then(|id| {
                            this.visible
                                .iter()
                                .position(|&ix| this.stickers[ix].id == id)
                        });
                    }
                    Err(error) => this.error = Some(format!("Could not load stickers: {error}")),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn create_sticker(&mut self, cx: &mut Context<Self>, kind: &StickerType) {
        if self.creating {
            return;
        }
        let size = match kind {
            StickerType::Markdown => MarkdownSticker::default_window_size(),
            StickerType::Command => CommandSticker::default_window_size(),
            StickerType::Timer => TimerSticker::default_window_size(),
            StickerType::Paint => PaintSticker::default_window_size(),
            StickerType::File => FileSticker::default_window_size(),
        };
        let title = match kind {
            StickerType::Markdown => "New text sticker",
            StickerType::Command => "New command sticker",
            StickerType::Timer => "New timer sticker",
            StickerType::Paint => "New sketch",
            StickerType::File => "New file sticker",
        };
        let detail = StickerDetail {
            id: 0,
            title: title.into(),
            content: String::new(),
            color: StickerColor::Gray,
            sticker_type: *kind,
            state: StickerState::Open,
            left: 100,
            top: 100,
            width: size.width,
            height: size.height,
            top_most: false,
            created_at: 0,
            updated_at: 0,
            display_id: None,
            display_uuid: None,
            virtual_desktop_id: None,
            native_left: None,
            native_top: None,
            native_width: None,
            native_height: None,
            preferred_display_uuid: None,
            placements: Vec::new(),
        };
        self.creating = true;
        self.error = None;
        cx.notify();
        let store = self.store.clone();
        let tx = self.sticker_events_sender.clone();
        cx.spawn(async move |entity, cx| {
            let result = async {
                let id = store.insert_sticker(detail).await?;
                let _ = tx.send(StickerWindowEvent::Created { id });
                StickerWindow::open_async(cx, tx, store, id).await
            }
            .await;
            let _ = entity.update(cx, |this, cx| {
                this.creating = false;
                if let Err(error) = result {
                    this.error = Some(format!("Could not create or open sticker: {error}"));
                }
                cx.notify();
            });
        })
        .detach();
    }
    /// One pointer/keyboard opening path, with honest rollback of optimistic state.
    fn open_sticker(&mut self, id: i64, cx: &mut Context<Self>) {
        let prior = self.stickers.iter().find(|s| s.id == id).map(|s| s.state);
        if let Some(s) = self.stickers.iter_mut().find(|s| s.id == id) {
            s.state = StickerState::Open;
            cx.notify();
        }
        let store = self.store.clone();
        let tx = self.sticker_events_sender.clone();
        cx.spawn(async move |entity, cx| {
            if let Err(error) =
                StickerWindow::open_async_with_options(cx, tx, store, id, true).await
            {
                let _ = entity.update(cx, |this, cx| {
                    if let Some(s) = this.stickers.iter_mut().find(|s| s.id == id)
                        && let Some(prior) = prior
                    {
                        s.state = prior;
                    }
                    this.error = Some(format!("Could not open sticker: {error}"));
                    cx.notify();
                });
            }
        })
        .detach();
    }
    fn selected_sticker_id(&self) -> Option<i64> {
        self.selected_index
            .and_then(|ix| self.visible.get(ix))
            .and_then(|&ix| self.stickers.get(ix))
            .map(|s| s.id)
    }
    fn handle_list_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx)
            || !self.list_focus.is_focused(window)
            || self.visible.is_empty()
        {
            return;
        }
        let count = self.visible.len();
        let current = self.selected_index.unwrap_or(0);
        let key = event.keystroke.key.to_lowercase();
        let next = match key.as_str() {
            "down" => Some(if self.selected_index.is_none() {
                0
            } else {
                current.saturating_add(1)
            }),
            "up" => Some(current.saturating_sub(1)),
            "home" => Some(0),
            "end" => Some(count - 1),
            "pagedown" => Some(current.saturating_add(PAGE_STEP)),
            "pageup" => Some(current.saturating_sub(PAGE_STEP)),
            _ => None,
        };
        if let Some(next) = next {
            let next = next.min(count - 1);
            self.selected_index = Some(next);
            self.list_scroll
                .scroll_to_item(next, ScrollStrategy::Nearest);
            cx.notify();
        } else if key == "enter" {
            if let Some(id) = self.selected_sticker_id() {
                self.open_sticker(id, cx);
            }
        } else if key == "delete" {
            if let Some(id) = self.selected_sticker_id()
                && let Some(sticker) = self.stickers.iter().find(|s| s.id == id)
            {
                self.delete_sticker(id, sticker.title.clone(), window, cx);
            }
        } else {
            return;
        }
        cx.stop_propagation();
        window.prevent_default();
    }
    fn delete_sticker(
        &mut self,
        id: i64,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.entity().downgrade();
        let store = self.store.clone();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let store = store.clone();
            let weak = weak.clone();
            dialog
                .title(format!("Delete “{title}”?"))
                .child("This permanently removes the sticker and its saved content.")
                .confirm()
                .ok_text("Delete")
                .ok_variant(crate::gpui_component::button::ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    let store = store.clone();
                    let weak = weak.clone();
                    cx.spawn(async move |cx| {
                        let result = store.delete_sticker(id).await;
                        let _ = weak.update(cx, |this, cx| {
                            match result {
                                Ok(()) => {
                                    this.load_request_token =
                                        this.load_request_token.wrapping_add(1);
                                    this.loading = false;
                                    StickerWindow::try_close(id, cx);
                                    this.selected_index = None;
                                    this.stickers.retain(|s| s.id != id);
                                    this.commands.remove(&id);
                                    this.rebuild_visible(cx);
                                }
                                Err(error) => {
                                    this.error = Some(format!("Could not delete sticker: {error}"))
                                }
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                    true
                })
        });
    }
    fn sort_button(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let owner = cx.entity();
        Button::new("sort")
            .ghost()
            .icon(match self.order {
                StickerOrderBy::CreatedAsc | StickerOrderBy::UpdatedAsc => IconName::SortAscending,
                _ => IconName::SortDescending,
            })
            .accessibility_label("Sort stickers")
            .tooltip("Sort stickers")
            .dropdown_menu(move |mut menu, window, cx| {
                let selected = owner.read(cx).order;
                for order in [
                    StickerOrderBy::UpdatedDesc,
                    StickerOrderBy::CreatedDesc,
                    StickerOrderBy::CreatedAsc,
                    StickerOrderBy::UpdatedAsc,
                ] {
                    menu = menu.item(
                        PopupMenuItem::new(order_label(order))
                            .checked(selected == order)
                            .on_click(window.listener_for(&owner, move |this, _, _, cx| {
                                this.order = order;
                                this.spawn_load_stickers(cx);
                            })),
                    );
                }
                menu
            })
            .into_any_element()
    }
    fn create_button(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let owner = cx.entity();
        Button::new("create")
            .ghost()
            .icon(IconName::Plus)
            .disabled(self.creating)
            .dropdown_menu(move |mut menu, window, _| {
                for (label, kind) in [
                    ("Text", StickerType::Markdown),
                    ("Command", StickerType::Command),
                    ("Timer", StickerType::Timer),
                    ("Sketch", StickerType::Paint),
                ] {
                    menu = menu.item(
                        PopupMenuItem::new(label)
                            .icon(sticker_type_icon(&kind))
                            .on_click(window.listener_for(&owner, move |this, _, _, cx| {
                                this.create_sticker(cx, &kind)
                            })),
                    );
                }
                menu
            })
            .into_any_element()
    }
    fn title_bar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .flex_shrink_0()
            .relative()
            .border_b_1()
            .border_color(cx.theme().border)
            .window_control_area(WindowControlArea::Drag)
            .child(
                v_flex()
                    .py_3()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .px_5()
                            .text_lg()
                            .font_semibold()
                            .child(self.section.label()),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .gap_5()
                            .pl_5()
                            .pr_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.section.description()),
                            )
                            .when(self.section != Section::Settings, |v| {
                                v.child(
                                    div().flex_1().min_w_40().max_w_64().child(
                                        Input::new(&self.query)
                                            .cleanable(true)
                                            .prefix(Icon::new(IconName::Search))
                                            .tab_index(0),
                                    ),
                                )
                            }),
                    ),
            )
            .child(
                h_flex()
                    .absolute()
                    .top_0()
                    .right_0()
                    .occlude()
                    .gap_1()
                    .child(
                        Button::new("refresh-manager")
                            .ghost()
                            .occlude()
                            .icon(IconName::Refresh)
                            .accessibility_label("Refresh stickers and settings")
                            .tooltip("Refresh stickers and settings")
                            .disabled(self.loading || self.settings_busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.error = None;
                                this.spawn_load_stickers(cx);
                                if this.section == Section::Settings {
                                    this.load_settings(cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("minimize")
                            .ghost()
                            .occlude()
                            .icon(IconName::Minus)
                            .accessibility_label("Minimize")
                            .tooltip("Minimize")
                            .on_click(|_, window, _| window.minimize_window()),
                    )
                    .child(
                        Button::new("close")
                            .ghost()
                            .occlude()
                            .icon(IconName::Close)
                            .accessibility_label("Quit Rustickers")
                            .tooltip("Quit Rustickers")
                            .on_click(|_, window, cx| {
                                Self::request_quit(window, cx);
                            }),
                    )
                    .when(self.section != Section::Settings, |v| {
                        v.child(self.sort_button(cx)).child(self.create_button(cx))
                    }),
            )
            .into_any_element()
    }
}
impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let compact = window.bounds().size.width < window.rem_size() * 48.;
        h_flex()
            .items_stretch()
            .size_full()
            .bg(cx.theme().background.opacity(crate::native::theme::WINDOW_SURFACE_OPACITY))
            .text_color(cx.theme().foreground)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if window.has_active_dialog(cx) {
                    return;
                }
                if this.section != Section::Settings
                    && (event.keystroke.modifiers.control || event.keystroke.modifiers.platform)
                    && event.keystroke.key.eq_ignore_ascii_case("f")
                {
                    this.query.focus_handle(cx).focus(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(self.navigation(compact, window, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.title_bar(cx))
                    .when_some(self.error.as_ref(), |v, error| {
                        v.child(
                            div()
                                .px_5()
                                .pb_3()
                                .child(Alert::error("manager-error", error.as_str())),
                        )
                    })
                    .child(if self.section == Section::Settings {
                        self.settings_view(cx)
                    } else {
                        self.collection(cx)
                    })
                    .child(
                        h_flex()
                            .h_9()
                            .flex_shrink_0()
                            .px_5()
                            .gap_2()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .when(self.loading, |v| v.child(Spinner::new().small()))
                            .child(if self.section == Section::Settings {
                                "Changes apply immediately".into()
                            } else {
                                format!(
                                    "{} stickers · {} on desktop",
                                    self.visible.len(),
                                    self.visible
                                        .iter()
                                        .filter(|&&ix| self.stickers[ix].state == StickerState::Open)
                                        .count()
                                )
                            })
                            .child(div().flex_1())
                            .when(self.section != Section::Settings, |v| {
                                v.child("↑ ↓ navigate · Enter opens")
                            }),
                    ),
            )
    }
}
fn sticker_type_icon(kind: &StickerType) -> IconName {
    match kind {
        StickerType::Markdown => IconName::DocumentText,
        StickerType::Command => IconName::Command,
        StickerType::Timer => IconName::Bell,
        StickerType::Paint => IconName::Paint,
        StickerType::File => IconName::Folder,
    }
}
fn order_label(order: StickerOrderBy) -> &'static str {
    match order {
        StickerOrderBy::CreatedDesc => "Newest created",
        StickerOrderBy::CreatedAsc => "Oldest created",
        StickerOrderBy::UpdatedDesc => "Recently updated",
        StickerOrderBy::UpdatedAsc => "Least recently updated",
    }
}
#[derive(Debug, PartialEq, Eq)]
enum CardActivity {
    Idle,
    Running,
    Scheduled(String),
}
impl CardActivity {
    fn of_command(running: bool, next: Option<String>) -> Self {
        match (running, next) {
            (true, _) => Self::Running,
            (_, Some(next)) => Self::Scheduled(next),
            _ => Self::Idle,
        }
    }
    fn status_line(&self, fallback: &str) -> String {
        match self {
            Self::Running => "Running".into(),
            Self::Scheduled(next) => format!("Next: {next}"),
            Self::Idle => fallback.into(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn running_status_takes_priority_over_schedule() {
        assert_eq!(
            CardActivity::of_command(true, Some("tomorrow".into())),
            CardActivity::Running
        );
    }
    #[test]
    fn idle_status_preserves_the_object_state() {
        assert_eq!(CardActivity::Idle.status_line("Closed"), "Closed");
    }
}
