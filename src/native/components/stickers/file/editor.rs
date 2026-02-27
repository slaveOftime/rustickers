use gpui::{
    Context, ImageSource, KeyDownEvent, MouseButton, MouseDownEvent, ObjectFit, Rgba, Window, div,
    img, prelude::*,
};
use gpui_component::{
    Sizable,
    button::Button,
    h_flex,
    input::{Input, InputState},
    scroll::ScrollableElement,
    text::TextView,
    v_flex,
};
use std::path::Path;

use super::preview::{FilePreview, build_preview};

const EDIT_HINT_TEXT: &str = "double-click to edit";

impl super::FileSticker {
    pub(super) fn spawn_refresh_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = (self.source_paths.len() == 1).then(|| self.source_paths[0].to_owned());
        let Some(source) = source else {
            self.preview = None;
            self.preview_editor = None;
            cx.notify();
            return;
        };

        let entity = cx.entity();
        window
            .spawn(cx, async move |cx| {
                let _ = entity.update_in(cx, move |this, window, cx| {
                    let bg = Rgba {
                        a: 0.5,
                        ..this.color.bg()
                    };
                    match build_preview(source.as_str(), bg, window, cx) {
                        Ok(preview) => {
                            if let Some(FilePreview::Audio { source_path }) = &preview {
                                let path = source_path.clone();
                                this.audio.handle = None;
                                this.audio.event_rx = None;
                                this.audio.is_playing = true;
                                this.audio.anim_loop_started = false;
                                let mut handle = super::audio::spawn_audio_thread(path.clone());
                                this.audio.event_rx = handle.take_event_rx();
                                this.audio.handle = Some(handle);
                                this.spawn_load_audio_metadata(path, cx);
                            }
                            this.preview = preview;
                            this.preview_editor = None;
                        }
                        Err(err) => {
                            this.preview = None;
                            this.preview_editor = None;
                            this.error = Some(err);
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
    }

    pub(super) fn start_preview_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let initial_content = match self
            .preview
            .as_ref()
            .and_then(FilePreview::editable_content)
        {
            Some(content) => content.to_string(),
            None => return,
        };
        let code_language = self
            .preview
            .as_ref()
            .and_then(FilePreview::code_language)
            .map(|language| language.to_string());

        self.preview_editor = Some(cx.new(move |cx| {
            let mut state = InputState::new(window, cx)
                .multi_line(true)
                .searchable(true)
                .placeholder("Edit file content, ctrl+s to save")
                .default_value(initial_content);
            if let Some(language) = code_language.as_ref() {
                state = state.code_editor(language);
            }
            state
        }));
        self.error = None;
        cx.notify();
    }

    pub(super) fn save_preview_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.preview_editor.as_ref() else {
            return;
        };
        let content = editor.read(cx).value().to_string();
        let save_path = match self
            .preview
            .as_ref()
            .and_then(FilePreview::editable_source)
            .map(Path::to_path_buf)
        {
            Some(path) => path,
            None => return,
        };

        match std::fs::write(&save_path, content.as_bytes()) {
            Ok(_) => {
                if let Some(preview) = self.preview.as_mut() {
                    preview.replace_content(content);
                }
                self.preview_editor = None;
                self.error = None;
                self.spawn_refresh_summaries(window, cx);
            }
            Err(err) => {
                self.error = Some(format!("Failed to save preview file: {err}"));
            }
        }
        cx.notify();
    }

    pub(super) fn handle_preview_double_click(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.click_count >= 2 {
            self.start_preview_edit(window, cx);
        }
    }

    fn maybe_edit_hint(&self, editable: bool) -> Option<gpui::AnyElement> {
        editable.then(|| {
            div()
                .absolute()
                .right_2()
                .top_8()
                .text_xs()
                .opacity(0.7)
                .child(EDIT_HINT_TEXT)
                .into_any_element()
        })
    }

    pub(super) fn preview_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if let Some(editor) = self.preview_editor.as_ref() {
            return v_flex()
                .size_full()
                .gap_1()
                .p_1()
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.modifiers.control
                        && event.keystroke.key.eq_ignore_ascii_case("s")
                    {
                        this.save_preview_edit(window, cx);
                    }
                }))
                .child(
                    Input::new(editor)
                        .size_full()
                        .bordered(false)
                        .bg(gpui::rgba(0x000000)),
                )
                .child(
                    h_flex().child(
                        Button::new("save-preview-file")
                            .label("Save (ctrl+s)")
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_preview_edit(window, cx);
                            })),
                    ),
                )
                .into_any_element();
        }

        match &self.preview {
            Some(FilePreview::Markdown {
                content, editable, ..
            }) => div()
                .p_2()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.handle_preview_double_click(event, window, cx);
                    }),
                )
                .child(
                    TextView::markdown("file-markdown", content)
                        .size_full()
                        .selectable(false)
                        .scrollable(true),
                )
                .when_some(
                    self.maybe_edit_hint(*editable && window.is_window_hovered()),
                    |view: gpui::Div, hint| view.child(hint),
                )
                .into_any_element(),
            Some(FilePreview::Text {
                content, editable, ..
            }) => {
                let mut base = div()
                    .p_2()
                    .size_full()
                    .text_sm()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.handle_preview_double_click(event, window, cx);
                        }),
                    )
                    .child(content.clone());
                if let Some(hint) = self.maybe_edit_hint(*editable && window.is_window_hovered()) {
                    base = base.child(hint);
                }
                base.overflow_scrollbar().into_any_element()
            }
            Some(FilePreview::Code {
                content,
                editable,
                language,
                ..
            }) => div()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.handle_preview_double_click(event, window, cx);
                    }),
                )
                .child(
                    TextView::markdown(
                        "file-code",
                        super::utils::wrap_code_as_markdown(language, content),
                    )
                    .size_full()
                    .selectable(false)
                    .scrollable(true),
                )
                .when_some(
                    self.maybe_edit_hint(*editable && window.is_window_hovered()),
                    |view: gpui::Div, hint| view.child(hint),
                )
                .into_any_element(),
            Some(FilePreview::Image(image)) => div()
                .size_full()
                .child(
                    img(ImageSource::Image(image.clone()))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element(),
            Some(FilePreview::WebView(webview)) => {
                div().size_full().child(webview.clone()).into_any_element()
            }
            Some(FilePreview::Audio { .. }) => self.audio_player_view(window, cx),
            None => div().child(self.summary_view()).into_any_element(),
        }
    }
}
