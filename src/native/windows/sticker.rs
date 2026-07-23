use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Context, IntoElement,
    MouseButton, Render, Rgba, Window, WindowBackgroundAppearance, WindowBounds, WindowControlArea,
    WindowKind, WindowOptions, div, prelude::*, px, rgba, size, transparent_black,
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
    sync::{RwLock, mpsc},
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
        GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SetWindowLongPtrW, SetWindowPos, WS_EX_TOOLWINDOW,
    },
};

use crate::model::content::FileStickerContent;
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
    store: ArcStickerStore,
    sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
    detail: StickerDetail,

    view: Box<dyn StickerView>,
    error: Option<String>,

    last_bounds: Option<WindowState>,
    last_bounds_change_at: Option<Instant>,
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
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_TOOLWINDOW.0 as isize);
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

        Self::open_file_preview_with_sources(cx, sticker_events_tx, store, sources)
    }

    pub fn open_file_preview_with_sources(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        sources: Vec<String>,
    ) -> anyhow::Result<()> {
        let default_size = FileSticker::default_window_size_for_sources(
            &sources.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        );

        let title = if sources.len() == 1 {
            source_title(&sources[0])
        } else {
            format!("{} files", sources.len())
        };

        let screen_size = cx
            .primary_display()
            .map(|d| d.bounds().size.map(|p| p.to_f64() as i32))
            .unwrap_or(size(1920, 1080));
        let left = (screen_size.width - default_size.width) / 2;
        let top = (screen_size.height - default_size.height) / 2;

        let content = FileStickerContent::from_sources(&sources).to_json();
        let detail = StickerDetail {
            id: generate_consistence_minus_id(&sources),
            title,
            state: StickerState::Open,
            left,
            top,
            width: default_size.width,
            height: default_size.height,
            top_most: true,
            color: StickerColor::Yellow,
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

    fn open_with_detail(
        cx: &mut App,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        store: ArcStickerStore,
        detail: StickerDetail,
        focus: bool,
    ) -> anyhow::Result<()> {
        let id = detail.id;
        if let Ok(mut open_stickers) = OPEN_STICKERS.write() {
            if id <= 0 {
                if let Some(pos) = open_stickers
                    .iter()
                    .position(|(open_id, _)| *open_id < 0 && *open_id != id)
                {
                    let (_, handle) = open_stickers.remove(pos);
                    let _ = handle.update(cx, |_, window, _| {
                        window.remove_window();
                    });
                }
            }

            if let Some((_, handle)) = open_stickers.iter().find(|(open_id, _)| *open_id == id) {
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

                let entity =
                    cx.new(|cx| StickerWindow::new(detail, store, sticker_events_tx, window, cx));
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
            open_stickers.push((id, handle.into()));
        }

        Ok(())
    }

    fn new(
        detail: StickerDetail,
        store: ArcStickerStore,
        sticker_events_tx: mpsc::Sender<StickerWindowEvent>,
        window: &mut Window,
        cx: &mut Context<StickerWindow>,
    ) -> Self {
        let title_val = detail.title.clone();
        let title = cx.new(|cx| InputState::new(window, cx).default_value(title_val));

        let mut view =
            Self::create_sticker_view(&detail, &store, window, cx, sticker_events_tx.clone());

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

        Self {
            store,
            detail,
            sticker_events_tx,
            view,
            last_bounds: None,
            last_bounds_change_at: None,
            error: None,
        }
    }

    fn create_sticker_view(
        detail: &StickerDetail,
        store: &ArcStickerStore,
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
                    content,
                    window,
                    cx,
                    sticker_events_tx.clone(),
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

    fn close(&mut self, cx: &mut gpui::App) {
        if !self.view.save_on_close(cx) {
            return;
        }

        let id = self.view.id(cx);
        let original_id = self.detail.id;
        let store = self.store.clone();
        let events = self.sticker_events_tx.clone();

        cx.spawn(async move |cx| {
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
        h_flex()
            .absolute()
            .left_0()
            .top_0()
            .right_0()
            .items_center()
            .child(div().size_full().cursor_move()) // Drag handle area
            .child(self.create_button(cx))
            .child(
                Button::new("close")
                    .bg(rgba(0x000000))
                    .border_0()
                    .cursor_pointer()
                    .icon(IconName::Close)
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            )
            .into_any_element()
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
        let color_options = h_flex()
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
            .absolute()
            .justify_end()
            .bottom_0()
            .left_0()
            .right_0()
            .p_2()
            .gap_2()
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
            .window_control_area(WindowControlArea::Drag)
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
                view.child(self.header_view(cx))
            })
            .when(window.is_window_hovered(), |view| {
                view.child(self.footer_view(cx))
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
