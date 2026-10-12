//! Recipient-only text encryption and password-wrapped private identities.
//! No unlocked identity is cached by this module.
use age::secrecy::ExposeSecret;
use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD as BASE64};
use rand::RngCore;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use serde::{Deserialize, Serialize};
use std::io::Read;
use zeroize::Zeroizing;

pub const CONTENT_PREFIX: &str = "RUSTICKERS_TEXT_V1\n";
const MEMORY_KIB: u32 = 65_536;
const ITERATIONS: u32 = 3;
const LANES: u32 = 2;
const MAX_ENVELOPE: usize = 64 * 1024 * 1024;

/// Persisted configuration contains only the public recipient and wrapped identity.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MasterKey {
    version: u8,
    key_id: String,
    recipient: String,
    salt: String,
    nonce: String,
    wrapped_identity: String,
    memory_kib: u32,
    iterations: u32,
    lanes: u32,
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasterKey")
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

impl MasterKey {
    pub fn create(password: &str) -> Result<Self> {
        ensure!(!password.is_empty(), "master password cannot be empty");
        let identity = age::x25519::Identity::generate();
        let salt = random::<16>();
        let nonce = random::<12>();
        let mut configuration = Self {
            version: 1,
            key_id: BASE64.encode(random::<16>()),
            recipient: identity.to_public().to_string(),
            salt: BASE64.encode(salt),
            nonce: BASE64.encode(nonce),
            wrapped_identity: String::new(),
            memory_kib: MEMORY_KIB,
            iterations: ITERATIONS,
            lanes: LANES,
        };
        let wrapping = derive(password, &salt)?;
        let encoded = identity.to_string();
        let mut buffer = Zeroizing::new(encoded.expose_secret().as_bytes().to_vec());
        cipher(&wrapping)?
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(configuration.binding()),
                &mut *buffer,
            )
            .map_err(|_| anyhow!("could not wrap private identity"))?;
        configuration.wrapped_identity = BASE64.encode(&*buffer);
        Ok(configuration)
    }

    pub fn parse(value: &str) -> Result<Self> {
        ensure!(value.len() <= 8192, "master key configuration is too large");
        let configuration: Self =
            serde_json::from_str(value).map_err(|_| anyhow!("invalid master key configuration"))?;
        configuration.validate()?;
        Ok(configuration)
    }

    pub fn serialize(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
    pub fn public_recipient(&self) -> &str {
        &self.recipient
    }

    fn binding(&self) -> Vec<u8> {
        format!(
            "rustickers-private-identity-v1\n{}\n{}",
            self.key_id, self.recipient
        )
        .into_bytes()
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && self.memory_kib == MEMORY_KIB
                && self.iterations == ITERATIONS
                && self.lanes == LANES,
            "unsupported master key format or KDF parameters"
        );
        ensure!(
            BASE64.decode(&self.key_id).map(|v| v.len()).unwrap_or(0) == 16,
            "invalid master key identity"
        );
        self.recipient
            .parse::<age::x25519::Recipient>()
            .map_err(|_| anyhow!("invalid public recipient"))?;
        Ok(())
    }

    pub(super) fn authorize(&self, password: &str) -> Result<age::x25519::Identity> {
        self.validate()?;
        let salt: [u8; 16] = BASE64
            .decode(&self.salt)?
            .try_into()
            .map_err(|_| anyhow!("invalid master key salt"))?;
        let nonce: [u8; 12] = BASE64
            .decode(&self.nonce)?
            .try_into()
            .map_err(|_| anyhow!("invalid master key nonce"))?;
        let wrapping = derive(password, &salt)?;
        let mut buffer = Zeroizing::new(BASE64.decode(&self.wrapped_identity)?);
        let key = cipher(&wrapping)?;
        let opened = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(self.binding()),
                &mut buffer,
            )
            .map_err(|_| anyhow!("incorrect master password or damaged key configuration"))?;
        let identity: age::x25519::Identity = std::str::from_utf8(opened)?
            .parse()
            .map_err(|_| anyhow!("invalid private identity"))?;
        ensure!(
            identity.to_public().to_string() == self.recipient,
            "private identity does not match recipient"
        );
        Ok(identity)
    }

    /// Verify a password without retaining its private identity.
    pub fn verify_password(&self, password: &str) -> Result<()> {
        self.authorize(password).map(drop)
    }

    /// Encrypt using only public configuration, binding plaintext to its sticker ID.
    pub fn seal(&self, sticker_id: i64, text: &str) -> Result<String> {
        self.validate()?;
        ensure!(
            sticker_id > 0 && text.len() < MAX_ENVELOPE / 2,
            "invalid or oversized text"
        );
        let recipient = self
            .recipient
            .parse::<age::x25519::Recipient>()
            .map_err(|_| anyhow!("invalid public recipient"))?;
        let mut payload = Zeroizing::new(Vec::new());
        payload.extend_from_slice(
            format!("rustickers-text-v1\n{sticker_id}\n{}\n", self.key_id).as_bytes(),
        );
        payload.extend_from_slice(text.as_bytes());
        let ciphertext = age::encrypt(&recipient, &payload).context("encrypt text")?;
        Ok(format!(
            "{CONTENT_PREFIX}{}\n{}",
            self.key_id,
            BASE64.encode(ciphertext)
        ))
    }

    pub fn open(
        &self,
        sticker_id: i64,
        envelope: &str,
        password: &str,
    ) -> Result<Zeroizing<String>> {
        ensure!(
            envelope.len() <= MAX_ENVELOPE,
            "encrypted text is too large"
        );
        let identity = self.authorize(password)?;
        self.open_authorized(sticker_id, envelope, &identity)
    }

    pub(super) fn open_authorized(
        &self,
        sticker_id: i64,
        envelope: &str,
        identity: &age::x25519::Identity,
    ) -> Result<Zeroizing<String>> {
        ensure!(
            envelope.len() <= MAX_ENVELOPE,
            "encrypted text is too large"
        );
        let (generation, ciphertext) = envelope
            .strip_prefix(CONTENT_PREFIX)
            .and_then(|s| s.split_once('\n'))
            .context("unsupported encrypted text format")?;
        ensure!(
            generation == self.key_id,
            "encrypted text belongs to a different master key"
        );
        let encrypted = BASE64
            .decode(ciphertext)
            .map_err(|_| anyhow!("damaged encrypted text"))?;
        let decryptor =
            age::Decryptor::new(encrypted.as_slice()).context("damaged encrypted text")?;
        // Never permit ciphertext to choose an expensive password-based age recipient.
        ensure!(
            !decryptor.is_scrypt(),
            "unsupported text encryption recipient"
        );
        let mut reader = decryptor
            .decrypt(std::iter::once(identity as &dyn age::Identity))
            .context("could not decrypt text")?;
        let mut payload = Zeroizing::new(Vec::new());
        reader
            .by_ref()
            .take(MAX_ENVELOPE as u64 + 1)
            .read_to_end(&mut payload)?;
        ensure!(payload.len() <= MAX_ENVELOPE, "decrypted text is too large");
        let binding = format!("rustickers-text-v1\n{sticker_id}\n{}\n", self.key_id);
        let text = std::str::from_utf8(&payload)?
            .strip_prefix(&binding)
            .context("encrypted text identity mismatch")?;
        Ok(Zeroizing::new(text.to_owned()))
    }
}

pub fn is_encrypted(content: &str) -> bool {
    content.starts_with(CONTENT_PREFIX)
}
/// Unsupported legacy formats must not become plaintext picker entries.
pub fn is_unsupported(content: &str) -> bool {
    content.trim_start().starts_with("RUSTICKERS_LOCKED_")
        || content.trim_start().starts_with("RUSTICKERS_VAULT_")
        || (content.starts_with("RUSTICKERS_TEXT_") && !is_encrypted(content))
}
fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
}
fn derive(password: &str, salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>> {
    let parameters = argon2::Params::new(MEMORY_KIB, ITERATIONS, LANES, Some(32))
        .map_err(|_| anyhow!("invalid KDF parameters"))?;
    let mut key = Zeroizing::new([0; 32]);
    argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        parameters,
    )
    .hash_password_into(password.as_bytes(), salt, key.as_mut())
    .map_err(|_| anyhow!("master password derivation failed"))?;
    Ok(key)
}
fn cipher(key: &[u8; 32]) -> Result<LessSafeKey> {
    Ok(LessSafeKey::new(
        UnboundKey::new(&AES_256_GCM, key).map_err(|_| anyhow!("invalid wrapping key"))?,
    ))
}
