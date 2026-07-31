use gpui::{
    Anchor, AnyElement, AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Context, IntoElement,
    KeystrokeEvent, MouseButton, Render, Rgba, Subscription, Window, WindowBackgroundAppearance,
    WindowBounds, WindowControlArea, WindowKind, WindowOptions, div, prelude::*, px, rgba, size,
    transparent_black,
};
use gpui_component::{
    ActiveTheme, Root,
    alert::Alert,
    button::Button,
    h_flex,
    input::{InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    v_flex,
};
use std::{
    path::PathBuf,
    sync::{
        Arc, RwLock,
        atomic::{AtomicI64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use url::Url;

#[cfg(target_os = "macos")]
use cocoa::appkit::{
    NSApplication, NSMainMenuWindowLevel, NSWindow, NSWindowButton, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
#[cfg(target_os = "macos")]
use cocoa::base::{YES, nil};
#[cfg(target_os = "macos")]
use objc::{msg_send, sel, sel_impl};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
#[cfg(target_os = "windows")]
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW, HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WS_EX_TOOLWINDOW,
        WS_SYSMENU,
    },
};

use crate::model::content::{CommandContent, FileStickerContent};
use crate::model::sticker::{StickerColor, StickerDetail, StickerState, StickerType};
use crate::native::components::{
    IconName,
    stickers::{
        command::CommandSticker, file::FileSticker, markdown::MarkdownSticker, paint::PaintSticker,
        timer::TimerSticker, *,
    },
};
use crate::native::file_manager;
use crate::native::windows::StickerWindowEvent;
use crate::storage::ArcStickerStore;

const BOUNDS_SAVE_DEBOUNCE: Duration = Duration::from_millis(200);

static OPEN_STICKERS: RwLock<Vec<(i64, AnyWindowHandle)>> = RwLock::new(Vec::new());
const SELECTION_OPEN_ID_MIN: i64 = i64::MAX / 2;
static NEXT_SELECTION_OPEN_ID: AtomicI64 = AtomicI64::new(i64::MAX);

#[derive(Clone)]
struct SelectionCarousel {
    stickers: Arc<Vec<StickerDetail>>,
    index: usize,
    selection: Arc<str>,
    left: i32,
    top: i32,
    display_id: Option<u32>,
}

#[derive(PartialEq, Clone, Debug)]
struct WindowState {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    display_id: Option<u32>,
    scale_factor: f32,
}

pub struct StickerWindow {
    open_id: i64,
    store: ArcStickerStore,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    detail: StickerDetail,

    view: Box<dyn StickerView>,
    error: Option<String>,

    last_bounds: Option<WindowState>,
    last_bounds_change_at: Option<Instant>,
    selection_carousel: Option<SelectionCarousel>,
    _keystroke_subscription: Option<Subscription>,
}

impl StickerWindow {
    #[cfg(target_os = "macos")]
    fn configure_native_window(window: &Window, top_most: bool) {
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };

        // `Stationary` opts sticker windows out of macOS's Show Desktop animation,
        // so clicking the desktop does not push them toward the screen edges.
        unsafe {
            let view = handle.ns_view.as_ptr() as cocoa::base::id;
            let native_window: cocoa::base::id = msg_send![view, window];
            if !native_window.is_null() {
                // GPUI currently omits NSResizableWindowMask when titlebar is None, even when
                // WindowOptions::is_resizable is true. Sticker windows are borderless, so restore
                // the native resize style explicitly.
                let style = native_window.styleMask();
                native_window.setStyleMask_(style | NSWindowStyleMask::NSResizableWindowMask);

                // Adding the native resize style can make AppKit recreate the standard titlebar
                // controls. Keep native edge resizing but hide the traffic-light buttons.
                for button_kind in [
                    NSWindowButton::NSWindowCloseButton,
                    NSWindowButton::NSWindowMiniaturizeButton,
                    NSWindowButton::NSWindowZoomButton,
                    NSWindowButton::NSWindowFullScreenButton,
                ] {
                    let button = native_window.standardWindowButton_(button_kind);
                    if !button.is_null() {
                        let _: () = msg_send![button, setHidden: YES];
                    }
                }

                let behavior = native_window.collectionBehavior();
                let mut behavior =
                    behavior | NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary;
                if top_most {
                    behavior |= NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
                        | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary;
                }
                native_window.setCollectionBehavior_(behavior);

                if top_most {
                    // GPUI's floating level is only slightly above normal windows. Use the
                    // status-window level and explicitly order the preview to the front so it
                    // appears on the active Space without activating the whole application.
                    native_window.setLevel_((NSMainMenuWindowLevel + 1) as _);

                    // The global hotkey fires while Finder (or another application) is active.
                    // AppKit may defer presentation of a newly created Metal window belonging to
                    // an inactive application until that application receives an event. Activate
                    // Rustickers first, then make this preview the key/front window.
                    let app = NSApplication::sharedApplication(nil);
                    app.activateIgnoringOtherApps_(YES);
                    let _: () = msg_send![native_window, makeKeyAndOrderFront: nil];
                    native_window.orderFrontRegardless();
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    fn configure_preview_window(window: &Window, top_most: bool) {
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };

        // Windows excludes tool windows from Win+D's minimize-all operation. Preserve all
        // existing extended styles because GPUI also uses them for rendering and activation.
        unsafe {
            let hwnd = HWND(handle.hwnd.get() as *mut _);
            let extended_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                extended_style | WS_EX_TOOLWINDOW.0 as isize,
            );

            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_SYSMENU.0 as isize));
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );

            if top_most {
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }

    pub async fn open_async(
        cx: &mut AsyncApp,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        id: i64,
    ) -> anyhow::Result<()> {
        if let Ok(open_stickers) = OPEN_STICKERS.read() {
            if let Some((_, handle)) = open_stickers.iter().find(|(open_id, _)| *open_id == id) {
                let _ = cx.update(|cx| {
                    handle.update(cx, |_, window, _| {
                        window.activate_window();
                    })
                })?;
                return Ok(());
            }
        }

        let detail = match store.get_sticker(id).await {
            Ok(detail) => detail,
            Err(err) => {
                return Err(anyhow::anyhow!("Failed to open sticker: {err:#}"));
            }
        };

        if detail.state != StickerState::Open
            && let Err(err) = store.update_sticker_state(id, StickerState::Open).await
        {
            return Err(anyhow::anyhow!(
                "Failed to update sticker state to open: {err:#}"
            ));
        }

        cx.update(|cx| Self::open_with_detail(cx, sticker_events_tx, store, detail, false))
    }

    pub fn open_file_preview(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
    ) -> anyhow::Result<()> {
        let selected_files = file_manager::selected_files_from_active_manager()?;
        let sources = if selected_files.is_empty() {
            clipboard_preview_source()
                .map(|source| vec![source])
                .unwrap_or_default()
        } else {
            selected_files
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect()
        };

        if sources.is_empty() {
            return Err(anyhow::anyhow!(
                "No file selected and clipboard does not contain a file path or URL"
            ));
        }

        Self::open_file_preview_with_sources(
            cx,
            sticker_events_tx,
            store,
            sources,
            None,
            None,
            None,
        )
    }

    pub fn open_file_preview_with_sources(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        sources: Vec<String>,
        width: Option<i32>,
        height: Option<i32>,
        color: Option<StickerColor>,
    ) -> anyhow::Result<()> {
        let default_size = FileSticker::default_window_size_for_sources(
            &sources.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        );

        let title = if sources.len() == 1 {
            source_title(&sources[0])
        } else {
            format!("{} files", sources.len())
        };

        let width = width.unwrap_or(default_size.width);
        let height = height.unwrap_or(default_size.height);

        let screen_size = cx
            .primary_display()
            .map(|d| d.bounds().size.map(|p| p.to_f64() as i32))
            .unwrap_or(size(1920, 1080));
        let left = (screen_size.width - width) / 2;
        let top = (screen_size.height - height) / 2;

        let content = FileStickerContent::from_sources(&sources).to_json();
        let detail = StickerDetail {
            id: generate_consistence_minus_id(&sources),
            title,
            state: StickerState::Open,
            left,
            top,
            width,
            height,
            top_most: true,
            color: color.unwrap_or(StickerColor::Yellow),
            sticker_type: StickerType::File,
            content,
            created_at: 0,
            updated_at: 0,
            display_id: cx.primary_display().map(|x| u64::from(x.id()) as u32),
        };

        Self::open_with_detail(cx, sticker_events_tx, store, detail, true)
    }

    pub fn try_close(id: i64, cx: &mut App) -> bool {
        tracing::info!("Trying to close sticker with id: {}", id);
        if let Ok(mut open_stickers) = OPEN_STICKERS.write() {
            if let Some(pos) = open_stickers.iter().position(|(open_id, _)| *open_id == id) {
                let (_, handle) = open_stickers.remove(pos);
                return handle
                    .update(cx, |_, window, _| {
                        window.remove_window();
                        true
                    })
                    .unwrap_or(false);
            }
        }
        false
    }

    pub fn swap_open_sticker_id(old_id: i64, new_id: i64) {
        if let Ok(mut open_stickers) = OPEN_STICKERS.write() {
            if let Some((open_id, _)) = open_stickers
                .iter_mut()
                .find(|(open_id, _)| *open_id == old_id)
            {
                *open_id = new_id;
            }
        }
    }

    pub fn open_with_detail(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        detail: StickerDetail,
        focus: bool,
    ) -> anyhow::Result<()> {
        let open_id = detail.id;
        Self::open_with_detail_and_selection(
            cx,
            sticker_events_tx,
            store,
            detail,
            focus,
            None,
            open_id,
            None,
        )
    }

    pub(crate) fn open_selection_carousel(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        stickers: Vec<StickerDetail>,
        selection: String,
    ) -> anyhow::Result<()> {
        let (left, top, display_id) = {
            let first = &stickers[0];
            (first.left, first.top, first.display_id)
        };
        let carousel = SelectionCarousel {
            stickers: Arc::new(stickers),
            index: 0,
            selection: Arc::from(selection),
            left,
            top,
            display_id,
        };
        Self::open_selection_slide(cx, sticker_events_tx, store, carousel, true)
    }

    fn open_selection_slide(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        carousel: SelectionCarousel,
        focus: bool,
    ) -> anyhow::Result<()> {
        let existing_open_id = OPEN_STICKERS.read().ok().and_then(|open_stickers| {
            open_stickers
                .iter()
                .find(|(open_id, _)| *open_id >= SELECTION_OPEN_ID_MIN)
                .map(|(open_id, _)| *open_id)
        });
        if let Some(existing_open_id) = existing_open_id {
            Self::try_close(existing_open_id, cx);
            cx.defer(move |cx| {
                if let Err(err) =
                    Self::open_selection_slide(cx, sticker_events_tx, store, carousel, focus)
                {
                    tracing::warn!(error = ?err, "Failed to replace selection carousel");
                }
            });
            return Ok(());
        }

        let mut detail = carousel.stickers[carousel.index].clone();
        detail.left = carousel.left;
        detail.top = carousel.top;
        detail.display_id = carousel.display_id;
        detail.top_most = true;
        let selection = carousel.selection.to_string();
        let open_id = NEXT_SELECTION_OPEN_ID.fetch_sub(1, Ordering::Relaxed);
        Self::open_with_detail_and_selection(
            cx,
            sticker_events_tx,
            store,
            detail,
            focus,
            Some(selection),
            open_id,
            Some(carousel),
        )
    }

    fn open_with_detail_and_selection(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        detail: StickerDetail,
        focus: bool,
        selection: Option<String>,
        open_id: i64,
        selection_carousel: Option<SelectionCarousel>,
    ) -> anyhow::Result<()> {
        if let Ok(mut open_stickers) = OPEN_STICKERS.write() {
            if selection_carousel.is_none() && open_id <= 0 {
                if let Some(pos) = open_stickers
                    .iter()
                    .position(|(existing_id, _)| *existing_id < 0 && *existing_id != open_id)
                {
                    let (_, handle) = open_stickers.remove(pos);
                    let _ = handle.update(cx, |_, window, _| {
                        window.remove_window();
                    });
                }
            }

            if let Some((_, handle)) = open_stickers
                .iter()
                .find(|(existing_id, _)| *existing_id == open_id)
            {
                handle.update(cx, |_, window, _| {
                    window.activate_window();
                })?;
                return Ok(());
            }
        }

        let min_size = match detail.sticker_type {
            StickerType::Timer => TimerSticker::min_window_size(),
            StickerType::Markdown => MarkdownSticker::min_window_size(),
            StickerType::Command => CommandSticker::min_window_size(),
            StickerType::Paint => PaintSticker::min_window_size(),
            StickerType::File => FileSticker::min_window_size(),
        };

        let current_size = if detail.width > 0 && detail.height > 0 {
            size(detail.width, detail.height)
        } else {
            match detail.sticker_type {
                StickerType::Timer => TimerSticker::default_window_size(),
                StickerType::Markdown => MarkdownSticker::default_window_size(),
                StickerType::Command => CommandSticker::default_window_size(),
                StickerType::Paint => PaintSticker::default_window_size(),
                StickerType::File => FileSticker::default_window_size(),
            }
        };

        let bounds = Bounds::new(
            gpui::point(px(detail.left as f32), px(detail.top as f32)),
            current_size.map(|x| px(x as f32)),
        );

        let display_id = detail.display_id.and_then(|saved_id| {
            cx.displays()
                .iter()
                .find(|d| u64::from(d.id()) as u32 == saved_id)
                .map(|d| d.id())
        });

        let top_most = detail.top_most;

        // There is issue which gpui does not restore exactly with the given bounds especially on other displays
        let handle = cx.open_window(
            WindowOptions {
                focus,
                display_id,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(min_size.map(|x| px(x as f32))),
                window_background: WindowBackgroundAppearance::Transparent,
                is_resizable: true,
                kind: if top_most {
                    WindowKind::Floating
                } else {
                    WindowKind::Normal
                },
                titlebar: None,
                ..Default::default()
            },
            |window, cx| {
                #[cfg(target_os = "macos")]
                StickerWindow::configure_native_window(window, top_most);
                #[cfg(target_os = "windows")]
                StickerWindow::configure_preview_window(window, top_most);

                let entity = cx.new(|cx| {
                    StickerWindow::new(
                        open_id,
                        detail,
                        store,
                        sticker_events_tx,
                        selection,
                        selection_carousel,
                        window,
                        cx,
                    )
                });
                cx.new(|cx| Root::new(entity, window, cx).bg(transparent_black().alpha(0.0)))
            },
        )?;

        if focus {
            // Opening a window from the global-hotkey callback can happen while Rustickers is
            // inactive. Defer activation until GPUI has committed the new native window;
            // ordering it during the open_window callback is too early on macOS.
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| {
                    #[cfg(target_os = "macos")]
                    StickerWindow::configure_native_window(window, top_most);
                    window.refresh();
                    window.activate_window();
                });
            });
        }

        if let Ok(mut open_stickers) = OPEN_STICKERS.write() {
            open_stickers.push((open_id, handle.into()));
        }

        Ok(())
    }

    fn new(
        open_id: i64,
        detail: StickerDetail,
        store: ArcStickerStore,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        selection: Option<String>,
        selection_carousel: Option<SelectionCarousel>,
        window: &mut Window,
        cx: &mut Context<StickerWindow>,
    ) -> Self {
        let title_val = detail.title.clone();
        let title = cx.new(|cx| InputState::new(window, cx).default_value(title_val));

        let mut view = Self::create_sticker_view(
            &detail,
            &store,
            selection,
            window,
            cx,
            sticker_events_tx.clone(),
        );

        view.set_color(cx, detail.color);

        cx.subscribe_in(&title, window, |this, input_state, event, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
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
            }
        })
        .detach();

        let keystroke_subscription = selection_carousel.as_ref().map(|_| {
            let entity = cx.weak_entity();
            cx.intercept_keystrokes(move |event, window, cx| {
                if !window.is_window_active() {
                    return;
                }
                if let Some(entity) = entity.upgrade() {
                    let _ = entity.update(cx, |this, cx| {
                        this.handle_selection_key(event, window, cx);
                    });
                }
            })
        });

        Self {
            open_id,
            store,
            detail,
            sticker_events_tx,
            view,
            last_bounds: None,
            last_bounds_change_at: None,
            selection_carousel,
            _keystroke_subscription: keystroke_subscription,
            error: None,
        }
    }

    fn create_sticker_view(
        detail: &StickerDetail,
        store: &ArcStickerStore,
        selection: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    ) -> Box<dyn StickerView> {
        let id = detail.id;
        let color = detail.color;
        let content = detail.content.as_str();
        let store = store.clone();

        match detail.sticker_type {
            StickerType::Timer => Box::new(StickerViewEntity::new(cx.new(|cx| {
                TimerSticker::new(
                    id,
                    color,
                    store,
                    content,
                    window,
                    cx,
                    sticker_events_tx.clone(),
                )
            }))),
            StickerType::Markdown => Box::new(StickerViewEntity::new(cx.new(|cx| {
                MarkdownSticker::new(
                    id,
                    color,
                    store,
                    content,
                    window,
                    cx,
                    sticker_events_tx.clone(),
                )
            }))),
            StickerType::Command => Box::new(StickerViewEntity::new(cx.new(|cx| {
                CommandSticker::new(
                    id,
                    color,
                    store,
                    detail.title.as_str(),
                    content,
                    window,
                    cx,
                    sticker_events_tx.clone(),
                    selection,
                )
            }))),
            StickerType::Paint => {
                Box::new(StickerViewEntity::new(cx.new(|_| {
                    PaintSticker::new(id, color, store, content, sticker_events_tx.clone())
                })))
            }
            StickerType::File => Box::new(StickerViewEntity::new(cx.new(|cx| {
                FileSticker::new(
                    id,
                    color,
                    store,
                    content,
                    window,
                    cx,
                    sticker_events_tx.clone(),
                )
            }))),
        }
    }

    fn set_error(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        cx.notify();
    }

    fn tick_bounds_state(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.view.id(cx) <= 0 {
            return;
        }

        let current = self.current_bounds(window, cx);
        let changed = self
            .last_bounds
            .as_ref()
            .map(|x| x != &current)
            .unwrap_or(true);

        if changed {
            self.last_bounds = Some(current);
            self.last_bounds_change_at = Some(Instant::now());
            window.request_animation_frame();
            return;
        }

        if let Some(changed_at) = self.last_bounds_change_at {
            if changed_at.elapsed() >= BOUNDS_SAVE_DEBOUNCE {
                self.last_bounds_change_at = None;
                self.change_bounds(window, cx);
            } else {
                window.request_animation_frame();
            }
        }
    }

    fn try_tick(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.last_bounds.is_none() {
            self.last_bounds = Some(self.current_bounds(window, cx));
        }
        self.tick_bounds_state(window, cx);
    }

    fn current_bounds(&self, window: &Window, cx: &Context<Self>) -> WindowState {
        let bounds = window.bounds();
        let display_id = window.display(cx).map(|x| u64::from(x.id()) as u32);
        let scale_factor = window.scale_factor() as f32;

        WindowState {
            left: bounds.left().to_f64() as i32,
            top: bounds.top().to_f64() as i32,
            width: bounds.size.width.to_f64() as i32,
            height: bounds.size.height.to_f64() as i32,
            display_id,
            scale_factor,
        }
    }

    fn change_bounds(&mut self, window: &Window, cx: &mut Context<Self>) {
        let bounds = self.current_bounds(window, cx);
        if bounds.left != self.detail.left
            || bounds.top != self.detail.top
            || bounds.width != self.detail.width
            || bounds.height != self.detail.height
            || bounds.display_id != self.detail.display_id
        {
            self.last_bounds = Some(bounds.clone());

            let id = self.view.id(cx);
            let store = self.store.clone();

            tracing::debug!("Save bounds state: {:?}", &bounds);

            cx.spawn(async move |this, cx| {
                if let Err(err) = store
                    .update_sticker_bounds(
                        id,
                        bounds.left,
                        bounds.top,
                        bounds.width,
                        bounds.height,
                        bounds.display_id,
                    )
                    .await
                {
                    let _ = this.update(cx, |this, cx| {
                        this.set_error(format!("Failed to save window bounds: {err}"), cx);
                    });
                } else {
                    let _ = this.update(cx, |this, _| {
                        this.detail.left = bounds.left;
                        this.detail.top = bounds.top;
                        this.detail.width = bounds.width;
                        this.detail.height = bounds.height;
                        this.detail.display_id = bounds.display_id;
                    });
                }
            })
            .detach();
        }
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

    fn close_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.change_bounds(window, cx);
        let open_id = self.open_id;
        cx.defer(move |cx| {
            Self::try_close(open_id, cx);
        });
    }

    fn navigate_selection(&mut self, offset: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(carousel) = self.selection_carousel.as_ref() else {
            return;
        };
        let count = carousel.stickers.len();
        if count <= 1 {
            return;
        }

        let next_index = (carousel.index as isize + offset).rem_euclid(count as isize) as usize;
        self.select_selection(next_index, window, cx);
    }

    fn select_selection(&mut self, next_index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(carousel) = self.selection_carousel.clone() else {
            return;
        };
        if next_index >= carousel.stickers.len() || next_index == carousel.index {
            return;
        }

        let bounds = self.current_bounds(window, cx);
        self.change_bounds(window, cx);
        let mut stickers = carousel.stickers.as_ref().clone();
        let current = &mut stickers[carousel.index];
        current.left = bounds.left;
        current.top = bounds.top;
        current.width = bounds.width;
        current.height = bounds.height;
        current.display_id = bounds.display_id;

        let next_id = stickers[next_index].id;
        let next_carousel = SelectionCarousel {
            stickers: Arc::new(stickers),
            index: next_index,
            selection: carousel.selection.clone(),
            left: bounds.left,
            top: bounds.top,
            display_id: bounds.display_id,
        };

        let store_for_lru = self.store.clone();
        cx.spawn(async move |_, _| {
            if let Err(err) = store_for_lru
                .touch_selection_lru(next_id, crate::utils::time::now_unix_millis())
                .await
            {
                tracing::warn!(next_id, error = ?err, "Failed to update selection carousel LRU");
            }
        })
        .detach();

        let mut next_detail = next_carousel.stickers[next_index].clone();
        next_detail.left = bounds.left;
        next_detail.top = bounds.top;
        next_detail.display_id = bounds.display_id;
        next_detail.top_most = true;
        let next_size = if next_detail.width > 0 && next_detail.height > 0 {
            size(px(next_detail.width as f32), px(next_detail.height as f32))
        } else {
            CommandSticker::default_window_size().map(|value| px(value as f32))
        };
        let mut next_view = Self::create_sticker_view(
            &next_detail,
            &self.store,
            Some(next_carousel.selection.to_string()),
            window,
            cx,
            self.sticker_events_tx.clone(),
        );
        next_view.set_color(cx, next_detail.color);

        self.detail = next_detail;
        self.view = next_view;
        self.selection_carousel = Some(next_carousel);
        self.last_bounds = None;
        self.last_bounds_change_at = None;
        window.resize(next_size);
        window.refresh();
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection_carousel.is_some() {
            self.close_selection(window, cx);
            return;
        }

        if !self.view.save_on_close(cx) {
            return;
        }

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

            let _ = cx.update(|cx| {
                if !Self::try_close(id, cx) {
                    Self::try_close(original_id, cx);
                }
            });
        })
        .detach();
    }

    fn header_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let extension = self.view.header_extension(cx);

        h_flex()
            .absolute()
            .left_0()
            .top_0()
            .right_0()
            .items_center()
            .cursor_grab()
            .window_control_area(WindowControlArea::Drag)
            .occlude()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when_some(extension, |view, extension| view.child(extension)),
            )
            .child(self.create_button(cx))
            .child(
                Button::new("close")
                    .bg(rgba(0x000000))
                    .border_0()
                    .cursor_pointer()
                    .icon(IconName::Close)
                    .occlude()
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
            )
            .into_any_element()
    }

    fn selection_navigation_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let carousel = self.selection_carousel.as_ref().unwrap();
        let root_entity = cx.entity();
        h_flex()
            .items_center()
            .justify_center()
            .gap_1()
            .child(
                Button::new(("selection-prev", self.open_id as u64))
                    .icon(IconName::ArrowLeft)
                    .bg(rgba(0x000000))
                    .border_0()
                    .cursor_pointer()
                    .occlude()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.navigate_selection(-1, window, cx);
                    })),
            )
            .child(div().px_1().child(format!(
                "{} / {}",
                carousel.index + 1,
                carousel.stickers.len()
            )))
            .child(
                Button::new(("selection-next", self.open_id as u64))
                    .icon(IconName::ArrowRight)
                    .bg(rgba(0x000000))
                    .border_0()
                    .cursor_pointer()
                    .occlude()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.navigate_selection(1, window, cx);
                    })),
            )
            .child(
                Button::new(("selection-menu", self.open_id as u64))
                    .icon(IconName::Folder)
                    .bg(rgba(0x000000))
                    .border_0()
                    .cursor_pointer()
                    .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, window, cx| {
                        let Some(carousel) = root_entity.read(cx).selection_carousel.clone() else {
                            return menu;
                        };

                        carousel
                            .stickers
                            .iter()
                            .enumerate()
                            .fold(menu, |menu, (index, sticker)| {
                                let root_entity = root_entity.clone();
                                menu.item(
                                    PopupMenuItem::new(selection_sticker_label(sticker))
                                        .checked(index == carousel.index)
                                        .on_click(window.listener_for(
                                            &root_entity,
                                            move |this, _, window, cx| {
                                                this.select_selection(index, window, cx);
                                            },
                                        )),
                                )
                            })
                    }),
            )
            .into_any_element()
    }

    fn handle_selection_key(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selection_carousel.is_none() {
            return;
        }

        match event.keystroke.key.as_str() {
            "escape" => self.close_selection(window, cx),
            "left" => self.navigate_selection(-1, window, cx),
            "right" => self.navigate_selection(1, window, cx),
            _ => return,
        }
        cx.stop_propagation();
        window.prevent_default();
    }

    fn create_sticker(&mut self, cx: &mut Context<Self>, sticker_type: &StickerType) {
        let size = match sticker_type {
            StickerType::Markdown => MarkdownSticker::default_window_size(),
            StickerType::Command => CommandSticker::default_window_size(),
            StickerType::Timer => TimerSticker::default_window_size(),
            StickerType::Paint => PaintSticker::default_window_size(),
            StickerType::File => FileSticker::default_window_size(),
        };

        let title = match sticker_type {
            StickerType::Markdown => "New Text Sticker",
            StickerType::Command => "New Command Sticker",
            StickerType::Timer => "New Timer Sticker",
            StickerType::Paint => "New Paint Sticker",
            StickerType::File => "New File Sticker",
        };

        let detail = StickerDetail {
            id: 0,
            title: title.to_string(),
            content: "".to_string(),
            color: StickerColor::Yellow,
            sticker_type: *sticker_type,
            state: StickerState::Open,
            left: 100,
            top: 100,
            width: size.width,
            height: size.height,
            top_most: false,
            created_at: 0,
            updated_at: 0,
            display_id: None,
        };

        let store = self.store.clone();
        let sticker_events_tx = self.sticker_events_tx.clone();
        cx.spawn(
            async move |entity, cx| match store.insert_sticker(detail).await {
                Ok(id) => {
                    let _ = sticker_events_tx.send(StickerWindowEvent::Created { id });

                    if let Err(err) =
                        StickerWindow::open_async(cx, sticker_events_tx.clone(), store.clone(), id)
                            .await
                    {
                        let _ = entity.update(cx, |this, cx| {
                            this.set_error(format!("Failed to open sticker window: {err:#}"), cx);
                        });
                    }
                }
                Err(err) => {
                    let _ = entity.update(cx, |this, cx| {
                        this.set_error(format!("Failed to create sticker: {err:#}"), cx);
                    });
                }
            },
        )
        .detach();
    }

    fn create_button(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let root_entity = cx.entity();
        Button::new("create")
            .border_0()
            .bg(rgba(0x00000000))
            .icon(IconName::Plus)
            .opacity(0.8)
            .dropdown_menu(move |menu, window, _| {
                let root_entity = root_entity.clone();
                menu.item(
                    PopupMenuItem::new("text")
                        .icon(sticker_type_icon(&StickerType::Markdown))
                        .on_click(window.listener_for(&root_entity, |this, _, _, cx| {
                            this.create_sticker(cx, &StickerType::Markdown);
                        })),
                )
                .item(
                    PopupMenuItem::new("timer")
                        .icon(sticker_type_icon(&StickerType::Timer))
                        .on_click(window.listener_for(&root_entity, |this, _, _, cx| {
                            this.create_sticker(cx, &StickerType::Timer);
                        })),
                )
                .item(
                    PopupMenuItem::new("command")
                        .icon(sticker_type_icon(&StickerType::Command))
                        .on_click(window.listener_for(&root_entity, |this, _, _, cx| {
                            this.create_sticker(cx, &StickerType::Command);
                        })),
                )
                .item(
                    PopupMenuItem::new("paint")
                        .icon(sticker_type_icon(&StickerType::Paint))
                        .on_click(window.listener_for(&root_entity, |this, _, _, cx| {
                            this.create_sticker(cx, &StickerType::Paint);
                        })),
                )
            })
            .into_any_element()
    }

    fn footer_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let extension = self.view.footer_extension(cx);
        let color_options = h_flex()
            .p_2()
            .gap_1()
            .children(StickerColor::ALL.iter().map(|&theme| {
                div()
                    .w(px(16.0))
                    .h(px(16.0))
                    .bg(theme.swatch())
                    .rounded_full()
                    .cursor_pointer()
                    .window_control_area(WindowControlArea::Drag)
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.change_color(theme, cx);
                        }),
                    )
            }));

        h_flex()
            .when(self.view.is_footer_absoute(cx), |v| v.absolute())
            .justify_end()
            .bottom_0()
            .left_0()
            .right_0()
            .gap_2()
            .items_center()
            .occlude()
            .cursor_grab()
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when_some(extension, |view, extension| view.child(extension)),
            )
            .when(
                self.selection_carousel
                    .as_ref()
                    .is_some_and(|carousel| carousel.stickers.len() > 1),
                |view| view.child(self.selection_navigation_view(cx)),
            )
            .when(!self.view.disable_color_picker(cx), move |v| {
                v.child(color_options)
            })
            .into_any_element()
    }
}

impl Render for StickerWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.try_tick(window, cx);

        window.set_rem_size(cx.theme().font_size);

        v_flex()
            .text_color(cx.theme().foreground)
            .font_family(cx.theme().font_family.clone())
            .relative()
            .size_full()
            .on_mouse_down(MouseButton::Left, |event, window, cx| {
                if !window.is_window_active() {
                    window.activate_window();
                }
                if event.click_count >= 2 {
                    cx.stop_propagation();
                    window.prevent_default();
                }
            })
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.change_bounds(window, cx);
                }),
            )
            .when(self.view.use_default_bg(cx), |view| {
                view.bg(Rgba {
                    a: 0.85,
                    ..self.detail.color.bg()
                })
            })
            .when_some(self.error.as_ref(), |view, msg| {
                view.child(
                    div()
                        .p_2()
                        .child(Alert::error("sticker-error", msg.as_str())),
                )
            })
            .child(self.view.element())
            .when(window.is_window_hovered(), |view| {
                view.child(self.header_view(cx)).child(self.footer_view(cx))
            })
    }
}

fn source_title(source: &str) -> String {
    if let Ok(url) = Url::parse(source) {
        if let Some(last_segment) = url.path_segments().and_then(|segments| segments.last())
            && !last_segment.is_empty()
        {
            return last_segment.to_string();
        }

        if let Some(host) = url.host_str()
            && !host.is_empty()
        {
            return host.to_string();
        }

        return source.to_string();
    }

    PathBuf::from(source)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string())
        .unwrap_or_else(|| source.to_string())
}

fn selection_sticker_label(sticker: &StickerDetail) -> String {
    let title = sticker.title.trim();
    if !title.is_empty() {
        return title.to_string();
    }

    let command = serde_json::from_str::<CommandContent>(&sticker.content)
        .map(|content| content.command)
        .unwrap_or_default();
    let command = command.split_whitespace().collect::<Vec<_>>().join(" ");
    if command.is_empty() {
        return "Untitled command".to_string();
    }

    let mut label: String = command.chars().take(60).collect();
    if command.chars().count() > 60 {
        label.push('…');
    }
    label
}

fn clipboard_preview_source() -> Option<String> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let text = clipboard.get_text().ok()?;
    let trimmed = text.trim();

    if trimmed.is_empty() {
        return None;
    }

    if crate::utils::url::is_url(trimmed) {
        return Some(trimmed.to_string());
    }

    let normalized = trimmed
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(trimmed)
        .to_string();

    if PathBuf::from(&normalized).exists() {
        return Some(normalized);
    }

    None
}

fn generate_consistence_minus_id(sources: &[String]) -> i64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    sources.hash(&mut hasher);
    let hash = hasher.finish() as i64;
    -hash.abs()
}

fn sticker_type_icon(sticker_type: &StickerType) -> IconName {
    match sticker_type {
        StickerType::Markdown => IconName::DocumentText,
        StickerType::Command => IconName::Command,
        StickerType::Timer => IconName::Bell,
        StickerType::Paint => IconName::Paint,
        StickerType::File => IconName::DocumentText,
    }
}
