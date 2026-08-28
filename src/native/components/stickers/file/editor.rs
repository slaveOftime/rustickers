use gpui::{Context, Window, prelude::*};
use gpui_component::input::EditorState;
use std::path::Path;

use super::preview::FilePreview;

impl super::FileSticker {
    pub(super) fn start_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked_content.is_some() {
            return;
        }
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
            let mut state = EditorState::new(window, cx)
                .placeholder("Edit file content, ctrl+s to save")
                .default_value(initial_content);
            if let Some(language) = code_language.as_ref() {
                state = state.language(language);
            }
            state.focus(window, cx);
            state
        }));
        self.error = None;
        cx.notify();
    }

    pub(super) fn save_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        if self.preview_editor.take().is_none() {
            return;
        }
        self.error = None;
        cx.notify();
    }
}
