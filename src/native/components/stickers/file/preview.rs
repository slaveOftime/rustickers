use gpui::{
    Context, Entity, Image, ImageSource, KeyDownEvent, MouseButton, MouseDownEvent, ObjectFit,
    Rgba, Window, div, img, prelude::*,
};
use gpui_component::{
    Sizable, button::Button, h_flex, input::Input, scroll::ScrollableElement, text::TextView,
    v_flex,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::native::components::webview::SimpleWebView;

const MAX_TEXT_PREVIEW_BYTES: usize = 1024 * 1024 * 5; // 5 MB

pub(super) enum FilePreview {
    Markdown {
        source_path: PathBuf,
        content: String,
        editable: bool,
    },
    Text {
        source_path: PathBuf,
        content: String,
        editable: bool,
    },
    Code {
        source_path: PathBuf,
        content: String,
        language: String,
        editable: bool,
    },
    Image(Arc<Image>),
    WebView(Entity<SimpleWebView>),
    Audio {
        source_path: PathBuf,
        state: super::audio::AudioState,
    },
}

impl FilePreview {
    pub fn new(
        source: &str,
        color: Rgba,
        window: &mut Window,
        cx: &mut Context<super::FileSticker>,
    ) -> Result<Option<FilePreview>, String> {
        if crate::utils::url::is_url(source) {
            return Ok(Some(FilePreview::WebView(cx.new(|cx| {
                let mut view = SimpleWebView::new(source, window, cx);
                view.set_bg(color, cx);
                view
            }))));
        }

        let path = Path::new(source);
        if path.is_dir() {
            return Ok(None);
        }

        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let ext = ext.as_str();

        if crate::utils::file::is_image_ext(ext) {
            let Some(format) = crate::utils::file::image_format_for_ext(ext) else {
                return Ok(None);
            };
            return match std::fs::read(path) {
                Ok(bytes) => Ok(Some(FilePreview::Image(Arc::new(gpui::Image::from_bytes(
                    format, bytes,
                ))))),
                Err(err) => Err(format!("Failed to load image preview: {err}")),
            };
        }

        if crate::utils::file::is_web_doc_ext(ext) || crate::utils::file::is_video_ext(ext) {
            return crate::utils::url::create_local_file_url(path)
                .map(|url| {
                    FilePreview::WebView(cx.new(|cx| {
                        let mut view = SimpleWebView::new(&url, window, cx);
                        view.set_bg(color, cx);
                        view
                    }))
                })
                .map(Some);
        }

        if crate::utils::file::is_audio_ext(ext) {
            return Ok(Some(FilePreview::Audio {
                source_path: path.to_path_buf(),
                state: super::audio::AudioState::default(),
            }));
        }

        if crate::utils::file::is_markdown_ext(ext) {
            let (content_result, editable) = read_text_for_preview(path);
            return content_result
                .map(|content| FilePreview::Markdown {
                    source_path: path.to_path_buf(),
                    content,
                    editable,
                })
                .map(Some)
                .map_err(|err| format!("Failed to read markdown preview: {err}"));
        }

        if crate::utils::file::is_code_ext(ext) {
            let language = crate::utils::file::markdown_language_for_ext(ext);
            return crate::utils::file::read_text_full(path)
                .map(|content| FilePreview::Code {
                    source_path: path.to_path_buf(),
                    content,
                    language: language.to_string(),
                    editable: true,
                })
                .map(Some)
                .map_err(|err| format!("Failed to read code preview: {err}"));
        }

        let (content_result, editable) = read_text_for_preview(path);
        content_result
            .map(|content| {
                if crate::utils::file::is_binary_text_content(content.as_str()) {
                    None
                } else {
                    Some(FilePreview::Text {
                        source_path: path.to_path_buf(),
                        content,
                        editable,
                    })
                }
            })
            .map_err(|err| format!("Failed to read text preview: {err}"))
    }

    pub(super) fn editable_source(&self) -> Option<&Path> {
        match self {
            Self::Markdown {
                source_path,
                editable: true,
                ..
            }
            | Self::Text {
                source_path,
                editable: true,
                ..
            }
            | Self::Code {
                source_path,
                editable: true,
                ..
            } => Some(source_path.as_path()),
            _ => None,
        }
    }

    pub(super) fn editable_content(&self) -> Option<&str> {
        match self {
            Self::Markdown {
                content,
                editable: true,
                ..
            }
            | Self::Text {
                content,
                editable: true,
                ..
            }
            | Self::Code {
                content,
                editable: true,
                ..
            } => Some(content.as_str()),
            _ => None,
        }
    }

    pub(super) fn code_language(&self) -> Option<&str> {
        match self {
            Self::Code { language, .. } => Some(language.as_str()),
            _ => None,
        }
    }

    pub(super) fn replace_content(&mut self, next_content: String) {
        match self {
            Self::Markdown { content, .. }
            | Self::Text { content, .. }
            | Self::Code { content, .. } => *content = next_content,
            _ => {}
        }
    }
}

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
                    match FilePreview::new(source.as_str(), bg, window, cx) {
                        Ok(preview) => {
                            this.preview = preview;
                            this.preview_editor = None;
                            if let Some(FilePreview::Audio { source_path, .. }) =
                                this.preview.as_ref()
                            {
                                this.load_audio(source_path.clone(), cx);
                            }
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

    fn handle_preview_double_click(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.click_count >= 2 {
            self.start_edit(window, cx);
        }
    }

    pub(super) fn preview_view(
        &mut self,
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
                        this.save_edit(window, cx);
                    } else if event.keystroke.key == "escape" {
                        this.preview_editor = None;
                        cx.notify();
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
                            .occlude()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_edit(window, cx);
                            })),
                    ),
                )
                .into_any_element();
        }

        match &self.preview {
            Some(FilePreview::Markdown { content, .. }) => div()
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
                .into_any_element(),
            Some(FilePreview::Text { content, .. }) => div()
                .p_2()
                .size_full()
                .text_sm()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.handle_preview_double_click(event, window, cx);
                    }),
                )
                .overflow_scrollbar()
                .child(content.clone())
                .into_any_element(),
            Some(FilePreview::Code {
                content, language, ..
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
            Some(FilePreview::Audio { .. }) => self.view(window, cx),
            None => div().child(self.summary_view()).into_any_element(),
        }
    }
}

fn read_text_for_preview(path: &Path) -> (std::io::Result<String>, bool) {
    let is_small_text = std::fs::metadata(path)
        .map(|metadata| metadata.len() <= MAX_TEXT_PREVIEW_BYTES as u64)
        .unwrap_or(false);
    let content = if is_small_text {
        crate::utils::file::read_text_full(path)
    } else {
        crate::utils::file::read_text_truncate(path, MAX_TEXT_PREVIEW_BYTES)
    };
    (content, is_small_text)
}
