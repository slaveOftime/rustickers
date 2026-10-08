//! Shared required public-title prompt for locking and renaming text.
use crate::gpui_component::{
    ActiveTheme, Disableable, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use gpui::{App, Context, Entity, Window, div, prelude::*};
type TitleCallback = Box<dyn FnOnce(String, &mut Window, &mut App)>;
struct TitleForm {
    input: Entity<InputState>,
    error: bool,
    callback: Option<TitleCallback>,
}
impl TitleForm {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.input.read(cx).value().to_string();
        if title.trim().is_empty() {
            self.error = true;
            cx.notify();
            return;
        }
        let callback = self.callback.take();
        window.close_dialog(cx);
        if let Some(callback) = callback {
            callback(title, window, cx);
        }
    }
}
pub(crate) fn prompt(
    window: &mut Window,
    cx: &mut App,
    locking: bool,
    initial: &str,
    callback: TitleCallback,
) {
    if window.has_active_dialog(cx) {
        return;
    }
    let form = cx.new(|cx| TitleForm {
        input: cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(initial.to_owned())
                .placeholder("Give this text a name")
        }),
        error: false,
        callback: Some(callback),
    });
    let input = form.read(cx).input.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let submit = form.clone(); let cancel = form.clone(); let keys = form.clone();
        let state = form.read(cx);
        dialog.title(if locking { "Name and lock text" } else { "Rename text" }).keyboard(false)
            .child(v_flex().gap_2().on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "enter" => keys.update(cx, |this,cx| this.submit(window,cx)),
                    "escape" => { window.close_dialog(cx); },
                    _ => return,
                }; cx.stop_propagation();
            }).child(div().child("Title (required)"))
              .child(Input::new(&state.input))
              .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Titles are public and searchable, even when the text is locked. Do not put secrets in the title."))
              .when(state.error, |body| body.child(div().text_sm().text_color(cx.theme().danger).child("Enter a title to continue."))))
            .footer(h_flex().gap_2()
                .child(Button::new("title-cancel").label("Cancel").on_click(move |_,window,cx| { cancel.update(cx, |this,_| this.callback=None); window.close_dialog(cx); }))
                .child(Button::new("title-submit").label(if locking { "Lock text" } else { "Save title" }).primary().disabled(state.callback.is_none())
                    .on_click(move |_,window,cx| submit.update(cx, |this,cx| this.submit(window,cx)))))
    });
    input.update(cx, |input, cx| input.focus(window, cx));
}
