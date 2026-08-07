//! Where a sticker window sits, and how that survives monitor and virtual-desktop changes.
//!
//! A sticker remembers one placement per monitor plus the monitor it prefers. Whenever the set of
//! connected monitors changes, the window is moved back onto its preferred monitor if that is
//! plugged in, or onto the primary one otherwise. Positions are stored and replayed in native
//! pixels; converting through GPUI's logical coordinates loses precision across monitors of
//! differing DPI.

use std::time::{Duration, Instant};

use gpui::{Context, Window};

use crate::model::sticker::{
    MAX_PLACEMENTS_PER_STICKER, StickerBounds, StickerDetail, StickerPlacement, prune_placements,
};
use crate::native::windows::placement::{
    DisplayEntry, NativeRect, ResolvedPlacement, resolve_placement,
};
use crate::utils::time::now_unix_millis;

use super::StickerWindow;
use super::platform::{self, ScaleResync};

/// How long a window has to sit still before its new bounds are written to the database.
pub(super) const BOUNDS_SAVE_DEBOUNCE: Duration = Duration::from_millis(200);

/// How long window moves are ignored after the set of connected monitors changes. Windows shoves
/// windows around itself during a dock or undock, and those moves must not be mistaken for the
/// user deliberately choosing a new monitor.
const DISPLAY_TRANSITION_GRACE: Duration = Duration::from_millis(1000);

/// How long after opening a sticker its restored placement keeps being re-asserted. Windows may
/// answer a cross-monitor move asynchronously, and GPUI applies the placement of a window opened
/// hidden only once it is shown, both of which land after the deferred restore has run.
const RESTORE_SETTLE: Duration = Duration::from_millis(1000);

/// Everything about a window's geometry that is worth persisting, sampled from the live window.
#[derive(PartialEq, Clone, Debug)]
pub(super) struct WindowState {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    display_id: Option<i64>,
    display_uuid: Option<String>,
    virtual_desktop_id: Option<String>,
    native_left: Option<i32>,
    native_top: Option<i32>,
    native_width: Option<i32>,
    native_height: Option<i32>,
    scale_factor: f32,
}

impl WindowState {
    fn native_rect(&self) -> Option<NativeRect> {
        Some(NativeRect {
            left: self.native_left?,
            top: self.native_top?,
            width: self.native_width?,
            height: self.native_height?,
        })
    }
}

/// A value that changes whenever a monitor is added, removed, moved or rescaled.
pub(super) fn display_fingerprint(displays: &[DisplayEntry]) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut entries: Vec<&DisplayEntry> = displays.iter().collect();
    entries.sort_by(|a, b| a.uuid.cmp(&b.uuid));

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for entry in entries {
        entry.uuid.hash(&mut hasher);
        entry.work_area.hash(&mut hasher);
        entry.scale_factor.to_bits().hash(&mut hasher);
        entry.is_primary.hash(&mut hasher);
    }
    hasher.finish()
}

/// The per-monitor placements of a sticker, falling back to the single placement older stickers
/// carry inline so they keep working before their first save.
fn placements_of(detail: &StickerDetail) -> Vec<StickerPlacement> {
    if !detail.placements.is_empty() {
        return detail.placements.clone();
    }

    let (Some(display_uuid), Some(left), Some(top), Some(width), Some(height)) = (
        detail.display_uuid.clone(),
        detail.native_left,
        detail.native_top,
        detail.native_width,
        detail.native_height,
    ) else {
        return Vec::new();
    };

    vec![StickerPlacement {
        display_uuid,
        display_id: detail.display_id,
        native_left: left,
        native_top: top,
        native_width: width,
        native_height: height,
        // The logical size was captured next to the native one, so their ratio is the DPI scale
        // of the monitor the rect came from, even once that monitor is gone.
        scale_factor: if detail.width > 0 && width > 0 {
            width as f32 / detail.width as f32
        } else {
            1.0
        },
        updated_at: detail.updated_at,
    }]
}

/// Where this sticker belongs given the monitors that exist right now.
pub(super) fn resolved_placement(
    detail: &StickerDetail,
    displays: &[DisplayEntry],
) -> Option<ResolvedPlacement> {
    resolve_placement(
        &placements_of(detail),
        detail.preferred_display_uuid.as_deref(),
        displays,
    )
}

/// The placement a freshly opened window should be nudged towards, and until when.
///
/// `open_with_options` asks the platform to place the window right after it is created, but that
/// move can still be undone afterwards: a DPI change may be answered asynchronously, and a window
/// opened hidden only gets GPUI's own placement once it is shown.
pub(super) fn pending_restore(
    detail: &StickerDetail,
    displays: &[DisplayEntry],
) -> Option<(NativeRect, Instant)> {
    resolved_placement(detail, displays)
        .map(|resolved| (resolved.rect, Instant::now() + RESTORE_SETTLE))
}

impl StickerWindow {
    /// One pass of the placement watchdog, run both from the polling timer and from `render`.
    ///
    /// Returns a scale correction that the caller must apply *after* it stops borrowing the
    /// window; see [`ScaleResync`].
    #[must_use]
    pub(super) fn tick_bounds_state(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ScaleResync> {
        if self.view.id(cx) <= 0 {
            return None;
        }

        let displays = platform::display_snapshot(cx);
        let fingerprint = display_fingerprint(&displays);
        if fingerprint != self.display_fingerprint {
            self.display_fingerprint = fingerprint;
            self.transition_until = Some(Instant::now() + DISPLAY_TRANSITION_GRACE);
            return self.relocate(window, cx, &displays);
        }

        if let Some(until) = self.transition_until {
            if Instant::now() < until {
                // Windows is still shuffling windows around; anything we observe now is its doing.
                self.reset_bounds_watch(window, cx);
                return None;
            }
            self.transition_until = None;
            // The layout has settled, so undo whatever Windows did to the window in the meantime.
            return self.relocate(window, cx, &displays);
        }

        if self.settle_restore(window, cx) {
            return None;
        }

        // Cheap safety net: a monitor change Windows reported late would otherwise leave the
        // sticker drawing itself at the wrong scale until the user clicks it.
        if let Some(resync) = platform::scale_resync(window) {
            self.redraw(window, cx);
            self.reset_bounds_watch(window, cx);
            return Some(resync);
        }

        let current = self.current_bounds(window, cx);
        if self.last_bounds.as_ref() != Some(&current) {
            self.last_bounds = Some(current);
            self.last_bounds_change_at = Some(Instant::now());
            return None;
        }

        if self
            .last_bounds_change_at
            .is_some_and(|changed_at| changed_at.elapsed() >= BOUNDS_SAVE_DEBOUNCE)
        {
            self.last_bounds_change_at = None;
            self.change_bounds(window, cx);
        }
        None
    }

    /// A resync raised here is dropped on purpose: rendering is the one moment the window must not
    /// be resized. The polling timer picks the correction up on its next pass.
    pub(super) fn try_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.last_bounds.is_none() {
            self.last_bounds = Some(self.current_bounds(window, cx));
        }
        let _ = self.tick_bounds_state(window, cx);
    }

    /// Put the sticker back where the current monitor layout says it belongs, then repaint it.
    fn relocate(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        displays: &[DisplayEntry],
    ) -> Option<ScaleResync> {
        self.reconcile_placement(window, displays);
        self.redraw(window, cx);
        self.reset_bounds_watch(window, cx);
        platform::scale_resync(window)
    }

    /// Redraw the sticker after it was moved between monitors.
    ///
    /// Relocating the window changes its size and, when the monitors differ in DPI, its scale
    /// factor. GPUI does not repaint on its own for a move it did not make, so the window would
    /// keep showing content laid out for the monitor the sticker just left.
    fn redraw(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.refresh();
        cx.notify();
    }

    /// Forget the pending debounce so the next tick compares against where the window is now.
    fn reset_bounds_watch(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.last_bounds = Some(self.current_bounds(window, cx));
        self.last_bounds_change_at = Some(Instant::now());
    }

    /// Move the sticker to wherever the current monitor layout says it belongs: back onto its
    /// preferred monitor when that is plugged in, otherwise onto the primary one.
    fn reconcile_placement(&mut self, window: &Window, displays: &[DisplayEntry]) {
        if !platform::window_is_visible(window) {
            return;
        }
        let Some(resolved) = resolved_placement(&self.detail, displays) else {
            return;
        };

        self.programmatic_rect = Some(resolved.rect);
        if platform::window_rect(window) == Some(resolved.rect) {
            return;
        }

        tracing::debug!(
            ?resolved,
            "Relocating sticker for the current monitor layout"
        );
        platform::apply_window_rect(window, resolved.rect);
    }

    /// Hold the freshly opened window on its restored placement for a moment.
    ///
    /// Returns `true` while the placement is still being asserted, so the caller skips its usual
    /// change detection and never persists a rectangle Windows or GPUI imposed on us.
    fn settle_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((rect, until)) = self.pending_restore else {
            return false;
        };
        if Instant::now() >= until {
            self.pending_restore = None;
            return false;
        }
        // A window that is still hidden has not received GPUI's own placement yet, so keep waiting.
        if !platform::window_is_visible(window) {
            return true;
        }
        let Some(actual) = platform::window_rect(window) else {
            self.pending_restore = None;
            return false;
        };
        if actual != rect {
            tracing::debug!(?rect, "Re-asserting the restored sticker placement");
            platform::apply_window_rect(window, rect);
            self.redraw(window, cx);
        }
        self.programmatic_rect = Some(rect);
        self.reset_bounds_watch(window, cx);
        true
    }

    /// Start asserting the sticker's placement again, for a window GPUI positions late.
    pub(super) fn rearm_restore(&mut self, cx: &mut Context<Self>) {
        let displays = platform::display_snapshot(cx);
        let Some((rect, until)) = pending_restore(&self.detail, &displays) else {
            return;
        };
        self.pending_restore = Some((rect, until));
        self.programmatic_rect = Some(rect);
    }

    fn current_bounds(&self, window: &Window, cx: &Context<Self>) -> WindowState {
        let bounds = window.bounds();
        let display = window.display(cx);
        let display_id = display.as_ref().map(|x| u64::from(x.id()) as i64);
        // Must match the key `display_snapshot` uses, or placements are saved under one name and
        // looked up under another.
        let display_uuid = display
            .as_ref()
            .map(|display| platform::display_uuid(display.as_ref()));
        let native_rect = platform::window_rect(window);

        WindowState {
            left: bounds.left().to_f64() as i32,
            top: bounds.top().to_f64() as i32,
            width: bounds.size.width.to_f64() as i32,
            height: bounds.size.height.to_f64() as i32,
            display_id,
            display_uuid,
            virtual_desktop_id: platform::current_virtual_desktop_id(window),
            native_left: native_rect.map(|rect| rect.left),
            native_top: native_rect.map(|rect| rect.top),
            native_width: native_rect.map(|rect| rect.width),
            native_height: native_rect.map(|rect| rect.height),
            scale_factor: window.scale_factor(),
        }
    }

    /// Persist the window's geometry, and remember the monitor it is on as the preferred one when
    /// the user is the one who put it there.
    pub(super) fn change_bounds(&mut self, window: &Window, cx: &mut Context<Self>) {
        let state = self.current_bounds(window, cx);
        if !self.detail_differs_from(&state) {
            return;
        }

        self.last_bounds = Some(state.clone());

        let native_rect = state.native_rect();
        // A move we made ourselves keeps the window on screen but says nothing about which monitor
        // the user wants, so it must not touch the per-monitor memory.
        let programmatic = matches!(
            (self.programmatic_rect, native_rect),
            (Some(expected), Some(actual)) if expected == actual
        );
        if !programmatic {
            self.programmatic_rect = None;
        }

        let placement = match (&state.display_uuid, native_rect) {
            (Some(display_uuid), Some(rect)) if !programmatic => Some(StickerPlacement {
                display_uuid: display_uuid.clone(),
                display_id: state.display_id,
                native_left: rect.left,
                native_top: rect.top,
                native_width: rect.width,
                native_height: rect.height,
                scale_factor: state.scale_factor,
                updated_at: now_unix_millis(),
            }),
            _ => None,
        };
        let primary_uuid = platform::display_snapshot(cx)
            .into_iter()
            .find(|display| display.is_primary)
            .map(|display| display.uuid);

        let id = self.view.id(cx);
        let store = self.store.clone();
        let bounds = StickerBounds {
            left: state.left,
            top: state.top,
            width: state.width,
            height: state.height,
            display_id: state.display_id,
            display_uuid: state.display_uuid.clone(),
            virtual_desktop_id: state.virtual_desktop_id.clone(),
            native_left: state.native_left,
            native_top: state.native_top,
            native_width: state.native_width,
            native_height: state.native_height,
        };

        tracing::debug!(programmatic, "Save bounds state: {:?}", &state);

        cx.spawn(async move |this, cx| {
            let mut result = store.update_sticker_bounds(id, bounds).await;

            if let (Ok(()), Some(placement)) = (&result, placement.clone()) {
                let preferred = placement.display_uuid.clone();
                result = store
                    .upsert_sticker_placement(id, placement, primary_uuid.clone())
                    .await;
                if result.is_ok() {
                    result = store
                        .update_sticker_preferred_display(id, Some(preferred))
                        .await;
                }
            }

            match result {
                Err(err) => {
                    let _ = this.update(cx, |this, cx| {
                        this.set_error(format!("Failed to save window bounds: {err}"), cx);
                    });
                }
                Ok(()) => {
                    let _ = this.update(cx, |this, _| {
                        this.apply_saved_bounds(state);
                        if let Some(placement) = placement {
                            this.remember_placement(placement, primary_uuid.as_deref());
                        }
                    });
                }
            }
        })
        .detach();
    }

    fn detail_differs_from(&self, state: &WindowState) -> bool {
        let detail = &self.detail;
        state.left != detail.left
            || state.top != detail.top
            || state.width != detail.width
            || state.height != detail.height
            || state.display_id != detail.display_id
            || state.display_uuid != detail.display_uuid
            || state.virtual_desktop_id != detail.virtual_desktop_id
            || state.native_left != detail.native_left
            || state.native_top != detail.native_top
            || state.native_width != detail.native_width
            || state.native_height != detail.native_height
    }

    fn apply_saved_bounds(&mut self, state: WindowState) {
        let detail = &mut self.detail;
        detail.left = state.left;
        detail.top = state.top;
        detail.width = state.width;
        detail.height = state.height;
        detail.display_id = state.display_id;
        detail.display_uuid = state.display_uuid;
        detail.virtual_desktop_id = state.virtual_desktop_id;
        detail.native_left = state.native_left;
        detail.native_top = state.native_top;
        detail.native_width = state.native_width;
        detail.native_height = state.native_height;
    }

    /// Mirror a saved placement into the in-memory copy so the next monitor change resolves
    /// against fresh data without another round trip to the database.
    fn remember_placement(&mut self, placement: StickerPlacement, protect_uuid: Option<&str>) {
        let mut placements = placements_of(&self.detail);
        self.detail.preferred_display_uuid = Some(placement.display_uuid.clone());

        match placements
            .iter_mut()
            .find(|existing| existing.display_uuid == placement.display_uuid)
        {
            Some(existing) => *existing = placement,
            None => placements.push(placement),
        }

        let stale = prune_placements(&placements, protect_uuid, MAX_PLACEMENTS_PER_STICKER);
        placements.retain(|placement| !stale.contains(&placement.display_uuid));
        self.detail.placements = placements;
    }
}
