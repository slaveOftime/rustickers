//! Sticker windows: one borderless, always-available window per sticker.
//!
//! The window itself only owns the chrome — the header, the colour picker and the close button —
//! and delegates its content to a [`StickerView`] chosen from the sticker's type. The rest of the
//! module is split by concern:
//!
//! * [`open`] tracks the live windows and creates new ones,
//! * [`bounds`] remembers where a sticker lives across monitor and virtual-desktop changes,
//! * [`platform`] holds every call that needs a native window handle,
//! * [`view`] draws the window.

mod bounds;
mod open;
mod platform;
mod view;

use std::{sync::mpsc, time::Instant};

use gpui::{AppContext, Context, Subscription, Window};
use gpui_component::input::{InputEvent, InputState};

use crate::model::sticker::{StickerColor, StickerDetail, StickerState};
use crate::native::components::stickers::{FOCUS_LOSS_RELOCK_DELAY, StickerView};
use crate::native::windows::{
    EscapeDismissTarget, StickerWindowEvent, placement::NativeRect,
    set_escape_dismiss_target_active, transient_topmost::TransientTopmost,
};
use crate::storage::ArcStickerStore;

use bounds::{BOUNDS_SAVE_DEBOUNCE, display_fingerprint};
use open::OpenOptions;

pub struct StickerWindow {
    open_id: i64,
    store: ArcStickerStore,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    detail: StickerDetail,

    view: Box<dyn StickerView>,
    error: Option<String>,

    last_bounds: Option<bounds::WindowState>,
    last_bounds_change_at: Option<Instant>,
    /// Identifies the monitor layout the sticker was last reconciled against.
    display_fingerprint: u64,
    /// While set, the monitor layout is still settling and window moves are not the user's doing.
    transition_until: Option<Instant>,
    /// The rect this app last forced onto the window. Seeing exactly this rect means the move was
    /// ours, so it must not be recorded as the user choosing a monitor.
    programmatic_rect: Option<NativeRect>,
    /// The placement the window is still being nudged towards right after it opened, together with
    /// the moment that stops.
    pending_restore: Option<(NativeRect, Instant)>,
    selection_run: bool,
    closing: bool,
    transient_topmost: TransientTopmost,
    protected_relock_generation: u64,
    _window_activation_subscription: Subscription,
}

impl StickerWindow {
    fn new(
        detail: StickerDetail,
        store: ArcStickerStore,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        options: OpenOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let OpenOptions {
            open_id,
            selection_run,
            hidden,
            ..
        } = options;

        let transient_topmost = TransientTopmost::new(
            detail.top_most && (selection_run || detail.id <= 0),
            !hidden,
        );

        let mut view = Self::create_sticker_view(
            &detail,
            &store,
            sticker_events_tx.clone(),
            &options,
            window,
            cx,
        );
        view.set_color(cx, detail.color);

        Self::watch_title(&detail, window, cx);
        Self::watch_bounds(window, cx);
        Self::watch_close_request(open_id, window, cx);
        Self::watch_dismiss_keystrokes(window, cx);
        let window_activation_subscription = Self::watch_activation(open_id, window, cx);

        let displays = platform::display_snapshot(cx);
        // Treat the placement we are about to assert as our own doing, so a botched restore is
        // never saved back as if the user had moved the sticker.
        let pending_restore = bounds::pending_restore(&detail, &displays);

        Self {
            open_id,
            store,
            detail,
            sticker_events_tx,
            view,
            last_bounds: None,
            last_bounds_change_at: None,
            display_fingerprint: display_fingerprint(&displays),
            transition_until: None,
            programmatic_rect: pending_restore.map(|(rect, _)| rect),
            pending_restore,
            selection_run,
            closing: false,
            error: None,
            transient_topmost,
            protected_relock_generation: 0,
            _window_activation_subscription: window_activation_subscription,
        }
    }

    /// Save the title as soon as the user confirms it with Enter.
    fn watch_title(detail: &StickerDetail, window: &mut Window, cx: &mut Context<Self>) {
        let title_value = detail.title.clone();
        let title = cx.new(|cx| InputState::new(window, cx).default_value(title_value));

        cx.subscribe_in(&title, window, |this, input_state, event, _, cx| {
            let InputEvent::PressEnter { .. } = event else {
                return;
            };
            let id = this.view.id(cx);
            let text = input_state.read(cx).value().to_string();
            let store = this.store.clone();
            let events = this.sticker_events_tx.clone();
            cx.spawn(async move |entity, cx| {
                if let Err(err) = store.update_sticker_title(id, text.clone()).await {
                    let _ = entity.update(cx, |this, cx| {
                        this.set_error(format!("Failed to save title: {err}"), cx);
                    });
                } else {
                    let _ = events.send(StickerWindowEvent::TitleChanged { id, title: text });
                }
            })
            .detach();
        })
        .detach();
    }

    /// Poll the window's geometry so moves, resizes and monitor changes are noticed.
    fn watch_bounds(window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                loop {
                    cx.background_executor().timer(BOUNDS_SAVE_DEBOUNCE).await;
                    match entity
                        .update_in(cx, |this, window, cx| this.tick_bounds_state(window, cx))
                    {
                        Err(_) => break,
                        // Applying this while the window was borrowed above would be ignored, so
                        // it deliberately happens between updates. See `platform::ScaleResync`.
                        Ok(Some(resync)) => resync.apply(),
                        Ok(None) => {}
                    }
                }
            })
            .detach();
    }

    /// Turn the platform's close request into our own save-then-close flow.
    fn watch_close_request(open_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            set_escape_dismiss_target_active(EscapeDismissTarget::Sticker(open_id), false);
            entity
                .update(cx, |this, cx| {
                    if this.closing {
                        true
                    } else {
                        this.close(window, cx);
                        false
                    }
                })
                .unwrap_or(true)
        });
    }

    /// Handle Ctrl+L and Escape before the focused sticker view sees them.
    fn watch_dismiss_keystrokes(window: &mut Window, cx: &mut Context<Self>) {
        let own_window_id = gpui::Window::window_handle(window).window_id();
        let entity = cx.weak_entity();
        cx.intercept_keystrokes(move |event, window, cx| {
            if gpui::Window::window_handle(window).window_id() != own_window_id
                || !window.is_window_active()
            {
                return;
            }

            let lock_shortcut =
                event.keystroke.modifiers.control && event.keystroke.key.eq_ignore_ascii_case("l");
            let escape = event.keystroke.key == "escape";
            if !lock_shortcut && !escape {
                return;
            }

            let handled = entity.upgrade().is_some_and(|entity| {
                entity.update(cx, |this, cx| {
                    if lock_shortcut {
                        return this.view.handle_lock_shortcut(window, cx);
                    }
                    if this.view.suppress_window_escape(cx) {
                        return false;
                    }
                    if !this.selection_run && this.view.id(cx) > 0 {
                        return false;
                    }

                    this.close(window, cx);
                    true
                })
            });
            if handled {
                cx.stop_propagation();
                window.prevent_default();
            }
        })
        .detach();
    }

    /// Re-lock protected content and drop the always-on-top behaviour once focus is lost.
    fn watch_activation(open_id: i64, window: &mut Window, cx: &mut Context<Self>) -> Subscription {
        let escape_target = EscapeDismissTarget::Sticker(open_id);
        cx.observe_window_activation(window, move |this, window, cx| {
            let active = window.is_window_active();
            this.protected_relock_generation = this.protected_relock_generation.wrapping_add(1);
            if !active && this.view.protected_content_visible(cx) {
                this.relock_protected_content_later(window, cx);
            }
            let closes_on_escape = this.selection_run || this.view.id(cx) <= 0;
            set_escape_dismiss_target_active(escape_target, closes_on_escape && active);
            if this.transient_topmost.update_activation(active) {
                platform::configure_window(window, false);
            }
        })
    }

    /// Re-lock protected content after a grace period, unless the window is focused again first.
    fn relock_protected_content_later(&self, window: &mut Window, cx: &mut Context<Self>) {
        let generation = self.protected_relock_generation;
        let entity = cx.entity();
        window
            .spawn(cx, async move |cx| {
                cx.background_executor()
                    .timer(FOCUS_LOSS_RELOCK_DELAY)
                    .await;
                let _ = entity.update_in(cx, |this, window, cx| {
                    if this.protected_relock_generation == generation
                        && !window.is_window_active()
                        && this.view.protected_content_visible(cx)
                    {
                        this.view.relock_protected_content(window, cx);
                    }
                });
            })
            .detach();
    }

    fn set_error(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        cx.notify();
    }

    fn change_color(&mut self, theme: StickerColor, cx: &mut Context<Self>) {
        self.detail.color = theme;
        self.view.set_color(cx, theme);
        cx.notify();

        let id = self.view.id(cx);
        if id <= 0 {
            return;
        }

        let store = self.store.clone();
        let events = self.sticker_events_tx.clone();
        cx.spawn(async move |entity, cx| {
            if let Err(err) = store
                .update_sticker_color(id, theme.as_str().to_string())
                .await
            {
                let _ = entity.update(cx, |this, cx| {
                    this.set_error(format!("Failed to save color: {err}"), cx);
                });
            } else {
                let _ = events.send(StickerWindowEvent::ColorChanged { id, color: theme });
            }
        })
        .detach();
    }

    /// Ask the sticker view to save, then mark the sticker closed and remove its window.
    fn close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }

        // A selection run has nothing to persist, its sticker only exists in memory.
        if self.selection_run {
            self.closing = true;
            let open_id = self.open_id;
            cx.defer(move |cx| {
                Self::try_close(open_id, cx);
            });
            return;
        }

        if !self.view.save_on_close(cx) {
            return;
        }
        self.closing = true;

        let id = self.view.id(cx);
        let original_id = self.detail.id;
        let store = self.store.clone();
        let events = self.sticker_events_tx.clone();

        cx.spawn(async move |_, cx| {
            if id > 0
                && let Err(err) = store.update_sticker_state(id, StickerState::Close).await
            {
                tracing::error!(id, error = %err, "Error saving state on close");
            }

            let _ = events.send(StickerWindowEvent::Closed { id });

            cx.update(|cx| {
                if !Self::try_close(id, cx) {
                    Self::try_close(original_id, cx);
                }
            });
        })
        .detach();
    }
}
