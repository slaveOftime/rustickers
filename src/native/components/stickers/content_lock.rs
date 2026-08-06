use std::{num::NonZeroU32, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use gpui::{AnyElement, Context, Entity, Focusable, KeyDownEvent, Window, div, prelude::*, px};
use gpui_component::{
    Disableable, StyledExt,
    button::Button,
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub(super) const LOCKED_CONTENT_PREFIX: &str = "RUSTICKERS_LOCKED_V1";
pub(crate) const FOCUS_LOSS_RELOCK_DELAY: Duration = Duration::from_secs(30);
const PBKDF2_ITERATIONS: u32 = 210_000;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct LockedContent {
    pub title: String,
    salt: String,
    nonce: String,
    ciphertext: String,
    iterations: u32,
}

pub(super) struct PreparedLock {
    pub locked: LockedContent,
    pub serialized: String,
}

pub(super) struct PreparedUnlock {
    pub content: String,
    pub title: String,
    pub updated_lock: Option<PreparedLock>,
}

pub(super) struct LockForm {
    title: Entity<InputState>,
    password: Entity<InputState>,
    confirm: Entity<InputState>,
}

impl LockForm {
    pub(super) fn new<T>(
        initial_title: String,
        password_placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Self {
        Self {
            title: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Sticker title")
                    .default_value(initial_title)
            }),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder(password_placeholder)
            }),
            confirm: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Confirm password")
            }),
        }
    }

    pub(super) fn reset_for_lock<T>(
        &self,
        title: String,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        self.title
            .update(cx, |input, cx| input.set_value(title, window, cx));
        self.clear_passwords(window, cx);
        self.focus_title(window, cx);
    }

    pub(super) fn set_title<T>(&self, title: String, window: &mut Window, cx: &mut Context<T>) {
        self.title
            .update(cx, |input, cx| input.set_value(title, window, cx));
    }

    pub(super) fn clear_passwords<T>(&self, window: &mut Window, cx: &mut Context<T>) {
        self.password
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.confirm
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    pub(super) fn focus_title<T>(&self, window: &mut Window, cx: &mut Context<T>) {
        self.title.update(cx, |input, cx| input.focus(window, cx));
    }

    pub(super) fn focus_password<T>(&self, window: &mut Window, cx: &mut Context<T>) {
        self.password
            .update(cx, |input, cx| input.focus(window, cx));
    }

    pub(super) fn prepare_lock<T>(
        &self,
        content: &str,
        cx: &Context<T>,
    ) -> Result<PreparedLock, String> {
        let title = self.title.read(cx).value().trim().to_string();
        let password = Zeroizing::new(self.password.read(cx).value().to_string());
        let confirm = Zeroizing::new(self.confirm.read(cx).value().to_string());
        if title.is_empty() {
            return Err("Sticker title cannot be empty".to_string());
        }
        if password != confirm {
            return Err("Passwords do not match".to_string());
        }
        prepare_lock(&title, password.as_str(), content)
    }

    pub(super) fn prepare_unlock<T>(
        &self,
        locked: &LockedContent,
        cx: &Context<T>,
    ) -> Result<PreparedUnlock, String> {
        let password = Zeroizing::new(self.password.read(cx).value().to_string());
        let content = locked.decrypt(password.as_str())?;
        let requested_title = self.title.read(cx).value().trim().to_string();
        let title = if requested_title.is_empty() {
            locked.title.clone()
        } else {
            requested_title
        };
        let updated_lock = if title == locked.title {
            None
        } else {
            Some(prepare_lock(&title, password.as_str(), &content)?)
        };
        Ok(PreparedUnlock {
            content,
            title,
            updated_lock,
        })
    }

    pub(super) fn prepare_unlock_forever<T>(
        &self,
        locked: &LockedContent,
        fallback_title: impl FnOnce(&str) -> String,
        cx: &Context<T>,
    ) -> Result<PreparedUnlock, String> {
        let password = Zeroizing::new(self.password.read(cx).value().to_string());
        let content = locked.decrypt(password.as_str())?;
        let requested_title = self.title.read(cx).value().trim().to_string();
        let title = if requested_title.is_empty() {
            fallback_title(&content)
        } else {
            requested_title
        };
        Ok(PreparedUnlock {
            content,
            title,
            updated_lock: None,
        })
    }

    pub(super) fn locking_view<T: 'static>(
        &self,
        id_prefix: &'static str,
        heading: &'static str,
        error: Option<&str>,
        busy: bool,
        cx: &mut Context<T>,
        on_cancel: fn(&mut T, &mut Window, &mut Context<T>),
        on_confirm: fn(&mut T, &mut Window, &mut Context<T>),
    ) -> AnyElement {
        let confirm = self.confirm.clone();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .p_4()
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                let handled = match event.keystroke.key.as_str() {
                    "escape" => {
                        on_cancel(this, window, cx);
                        true
                    }
                    "enter" if confirm.read(cx).focus_handle(cx).is_focused(window) => {
                        on_confirm(this, window, cx);
                        true
                    }
                    _ => false,
                };
                if handled {
                    cx.stop_propagation();
                    window.prevent_default();
                }
            }))
            .child(
                self.lock_panel(
                    heading,
                    error,
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new(format!("{id_prefix}-cancel-lock"))
                                .label("Cancel")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    on_cancel(this, window, cx);
                                })),
                        )
                        .child(
                            Button::new(format!("{id_prefix}-confirm-lock"))
                                .label("Lock")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    on_confirm(this, window, cx);
                                })),
                        )
                        .into_any_element(),
                ),
            )
            .into_any_element()
    }

    pub(super) fn locked_view<T: 'static>(
        &self,
        id_prefix: &'static str,
        error: Option<&str>,
        busy: bool,
        cx: &mut Context<T>,
        on_cancel: fn(&mut T, &mut Window, &mut Context<T>),
        on_unlock: fn(&mut T, &mut Window, &mut Context<T>),
        on_unlock_forever: fn(&mut T, &mut Window, &mut Context<T>),
    ) -> AnyElement {
        let password = self.password.clone();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .p_4()
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                let handled = match event.keystroke.key.as_str() {
                    "enter" if password.read(cx).focus_handle(cx).is_focused(window) => {
                        on_unlock(this, window, cx);
                        true
                    }
                    "escape" => {
                        on_cancel(this, window, cx);
                        true
                    }
                    _ => false,
                };
                if handled {
                    cx.stop_propagation();
                    window.prevent_default();
                }
            }))
            .child(
                self.unlock_panel(
                    error,
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new(format!("{id_prefix}-unlock"))
                                .label("Unlock")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    on_unlock(this, window, cx);
                                })),
                        )
                        .child(
                            Button::new(format!("{id_prefix}-unlock-forever"))
                                .label("Unlock forever")
                                .tooltip("Remove password protection and save as plain text")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    on_unlock_forever(this, window, cx);
                                })),
                        )
                        .into_any_element(),
                ),
            )
            .into_any_element()
    }

    fn lock_panel(
        &self,
        heading: &'static str,
        error: Option<&str>,
        actions: AnyElement,
    ) -> AnyElement {
        v_flex()
            .w_full()
            .max_w(px(360.0))
            .gap_2()
            .items_center()
            .child(div().text_lg().font_bold().child(heading))
            .child(Input::new(&self.title).w_full())
            .child(Input::new(&self.password).w_full())
            .child(Input::new(&self.confirm).w_full())
            .when_some(error, |view, error| {
                view.child(
                    div()
                        .text_sm()
                        .text_center()
                        .text_color(gpui::red())
                        .child(error.to_string()),
                )
            })
            .child(actions)
            .into_any_element()
    }

    fn unlock_panel(&self, error: Option<&str>, actions: AnyElement) -> AnyElement {
        v_flex()
            .w_full()
            .max_w(px(340.0))
            .gap_2()
            .items_center()
            .child(Input::new(&self.title).w_full())
            .child(Input::new(&self.password).w_full())
            .child(actions)
            .when_some(error, |view, error| {
                view.child(
                    div()
                        .text_sm()
                        .text_center()
                        .text_color(gpui::red())
                        .child(error.to_string()),
                )
            })
            .into_any_element()
    }
}

fn prepare_lock(title: &str, password: &str, content: &str) -> Result<PreparedLock, String> {
    let locked = LockedContent::encrypt(title, password, content)?;
    let serialized = locked.serialize()?;
    Ok(PreparedLock { locked, serialized })
}

impl LockedContent {
    pub(super) fn parse(content: &str) -> Result<Option<Self>, String> {
        let Some(payload) = content.strip_prefix(LOCKED_CONTENT_PREFIX) else {
            return Ok(None);
        };
        let payload = payload
            .strip_prefix('\n')
            .ok_or_else(|| "Locked content header is malformed".to_string())?;
        let locked: Self = serde_json::from_str(payload)
            .map_err(|err| format!("Locked content metadata is invalid: {err}"))?;
        locked.validate()?;
        Ok(Some(locked))
    }

    pub(super) fn encrypt(title: &str, password: &str, content: &str) -> Result<Self, String> {
        if password.is_empty() {
            return Err("Password cannot be empty".to_string());
        }

        let title = title.trim();
        let random = SystemRandom::new();
        let mut salt = [0_u8; SALT_LEN];
        let mut nonce = [0_u8; NONCE_LEN];
        random
            .fill(&mut salt)
            .map_err(|_| "Failed to generate encryption salt".to_string())?;
        random
            .fill(&mut nonce)
            .map_err(|_| "Failed to generate encryption nonce".to_string())?;

        let salt_text = STANDARD_NO_PAD.encode(salt);
        let mut ciphertext = Zeroizing::new(content.as_bytes().to_vec());
        let key = encryption_key(password, &salt, PBKDF2_ITERATIONS)?;
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad(title, &salt_text, PBKDF2_ITERATIONS).as_bytes()),
            &mut *ciphertext,
        )
        .map_err(|_| "Failed to encrypt content".to_string())?;

        Ok(Self {
            title: title.to_string(),
            salt: salt_text,
            nonce: STANDARD_NO_PAD.encode(nonce),
            ciphertext: STANDARD_NO_PAD.encode(&*ciphertext),
            iterations: PBKDF2_ITERATIONS,
        })
    }

    pub(super) fn decrypt(&self, password: &str) -> Result<String, String> {
        self.validate()?;
        let salt = decode_exact::<SALT_LEN>(&self.salt, "salt")?;
        let nonce = decode_exact::<NONCE_LEN>(&self.nonce, "nonce")?;
        let mut ciphertext = Zeroizing::new(
            STANDARD_NO_PAD
                .decode(&self.ciphertext)
                .map_err(|_| "Locked content ciphertext is invalid".to_string())?,
        );
        let key = encryption_key(password, &salt, self.iterations)?;
        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad(&self.title, &self.salt, self.iterations).as_bytes()),
                &mut *ciphertext,
            )
            .map_err(|_| "Incorrect password or damaged locked content".to_string())?;
        String::from_utf8(plaintext.to_vec())
            .map_err(|_| "Unlocked content is not valid UTF-8 text".to_string())
    }

    pub(super) fn serialize(&self) -> Result<String, String> {
        serde_json::to_string(self)
            .map(|payload| format!("{LOCKED_CONTENT_PREFIX}\n{payload}"))
            .map_err(|err| format!("Failed to serialize locked content: {err}"))
    }

    fn validate(&self) -> Result<(), String> {
        if self.iterations != PBKDF2_ITERATIONS {
            return Err("Unsupported locked content key settings".to_string());
        }
        decode_exact::<SALT_LEN>(&self.salt, "salt")?;
        decode_exact::<NONCE_LEN>(&self.nonce, "nonce")?;
        if self.ciphertext.is_empty() {
            return Err("Locked content ciphertext is empty".to_string());
        }
        Ok(())
    }
}

fn aad(title: &str, salt: &str, iterations: u32) -> String {
    format!("{LOCKED_CONTENT_PREFIX}\0{title}\0{salt}\0{iterations}")
}

fn decode_exact<const N: usize>(value: &str, field: &str) -> Result<[u8; N], String> {
    let decoded = STANDARD_NO_PAD
        .decode(value)
        .map_err(|_| format!("Locked content {field} is invalid"))?;
    decoded
        .try_into()
        .map_err(|_| format!("Locked content {field} has an invalid length"))
}

fn encryption_key(password: &str, salt: &[u8], iterations: u32) -> Result<LessSafeKey, String> {
    let mut key_bytes = Zeroizing::new([0_u8; KEY_LEN]);
    let iterations = NonZeroU32::new(iterations)
        .ok_or_else(|| "Locked content key settings are invalid".to_string())?;
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        iterations,
        salt,
        password.as_bytes(),
        &mut *key_bytes,
    );
    UnboundKey::new(&aead::CHACHA20_POLY1305, &*key_bytes)
        .map(LessSafeKey::new)
        .map_err(|_| "Failed to initialize content encryption".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_content_round_trips() {
        let locked = LockedContent::encrypt("Private note", "correct horse", "# Secret\nBody")
            .expect("encrypt content");
        let serialized = locked.serialize().expect("serialize content");
        let parsed = LockedContent::parse(&serialized)
            .expect("parse content")
            .expect("locked content");

        assert_eq!(parsed.title, "Private note");
        assert_eq!(
            parsed.decrypt("correct horse").expect("decrypt content"),
            "# Secret\nBody"
        );
    }

    #[test]
    fn wrong_password_does_not_expose_content() {
        let locked =
            LockedContent::encrypt("Private note", "right", "secret").expect("encrypt content");
        assert!(locked.decrypt("wrong").is_err());
    }

    #[test]
    fn plain_content_is_not_detected_as_locked() {
        assert!(LockedContent::parse("ordinary text").unwrap().is_none());
    }

    #[test]
    fn metadata_tampering_is_detected() {
        let mut locked =
            LockedContent::encrypt("Original", "password", "secret").expect("encrypt content");
        locked.title = "Changed".to_string();
        assert!(locked.decrypt("password").is_err());
    }

    #[test]
    fn title_whitespace_is_normalized_before_authentication() {
        let locked = LockedContent::encrypt("  Private note  ", "password", "secret")
            .expect("encrypt content");
        assert_eq!(locked.title, "Private note");
        assert_eq!(locked.decrypt("password").unwrap(), "secret");
    }
}
