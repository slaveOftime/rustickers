//! Master-password vault for protecting clipboard rows.
//!
//! The vault holds a single 32-byte data key that encrypts the plaintext of protected rows. The
//! data key is wrapped by an Argon2id-derived key from the master password; the wrapped blob is
//! the only thing persisted on disk. Plaintext keys never leave memory in any form other than a
//! zeroising wrapper.
//!
//! Why Argon2id and not PBKDF2: PBKDF2 in the existing sticker lock is fine for
//! per-sticker passwords that the user changes often, but a single long-lived master password
//! deserves a memory-hard KDF so an attacker who steals the database cannot brute-force common
//! passphrases. The sticker lock is intentionally kept as-is for backwards compatibility.

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use rand::RngCore;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

/// Current vault envelope format. Increment when key derivation or seal format changes.
pub const VAULT_VERSION: u8 = 1;
/// Memory cost (KiB) for Argon2id. 64 MiB is a 2026-class default — enough to deter GPU brute
/// force, light enough for a background task on a low-end laptop.
pub const ARGON2_M_COST_KIB: u32 = 65_536;
/// Iterations. Tune with `t_cost` until interactive unlock is < 250 ms on older hardware.
pub const ARGON2_T_COST: u32 = 3;
/// Parallelism. Threads count for Argon2id. Two hides GPU-friendly patterns.
pub const ARGON2_P_COST: u32 = 2;
/// Salt length, raw bytes.
pub const SALT_LEN: usize = 16;
/// Nonce length, raw bytes (AES-GCM uses 96-bit IVs).
pub const NONCE_LEN: usize = 12;

/// Prefix prepended to the wrapped data key so a future migration can detect version 1.
pub const VAULT_PREFIX: &str = "RUSTICKERS_VAULT_V1";

/// On-disk representation of the master password's data-key-encryption key.
#[derive(Clone, Serialize, Deserialize)]
pub struct VaultEnvelope {
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl std::fmt::Debug for VaultEnvelope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VaultEnvelope")
            .field("salt", &"[redacted]")
            .field("nonce", &self.nonce)
            .field("ciphertext", &"[redacted]")
            .field("m_cost", &self.m_cost)
            .field("t_cost", &self.t_cost)
            .field("p_cost", &self.p_cost)
            .finish()
    }
}

impl VaultEnvelope {
    /// Helper used by [`VaultController`]; same body as [`Self::serialize`] but exposed as an
    /// associated function so callers can write `VaultEnvelope::serialize_string(&envelope)`.
    pub fn serialize_string(envelope: &VaultEnvelope) -> Result<String> {
        envelope.serialize()
    }

    pub fn serialize(&self) -> Result<String> {
        let json = serde_json::to_string(self).context("encode vault envelope")?;
        Ok(format!("{VAULT_PREFIX}\n{json}"))
    }

    pub fn parse(value: &str) -> Result<Self> {
        let trimmed = value.trim_start();
        let Some(payload) = trimmed.strip_prefix(VAULT_PREFIX) else {
            bail!("vault envelope uses an unknown version prefix");
        };
        let envelope: VaultEnvelope =
            serde_json::from_str(payload.trim()).context("decode vault envelope JSON")?;
        Ok(envelope)
    }
}

/// In-memory state when the vault is unlocked: the data key wrapped in a [`Zeroizing`] buffer.
#[derive(Clone)]
pub struct UnlockedVault {
    envelope: VaultEnvelope,
    data_key: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for UnlockedVault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UnlockedVault")
            .field("envelope", &self.envelope)
            .field("data_key", &"[redacted]")
            .finish()
    }
}

impl UnlockedVault {
    /// Persist the envelope on disk, since settings live in `clipboard_settings.kv` for v1.
    pub fn envelope(&self) -> &VaultEnvelope {
        &self.envelope
    }

    /// Encrypt `plaintext` for storage on a protected row. Returns base64-encoded ciphertext
    /// with the AES-GCM nonce prepended so callers don't have to track the nonce separately.
    pub fn seal(&self, plaintext: &[u8]) -> Result<String> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let unbound = UnboundKey::new(&AES_256_GCM, self.data_key.as_ref())
            .map_err(|_| anyhow!("invalid AES-256-GCM key"))?;
        let key = LessSafeKey::new(unbound);
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let mut buffer = Zeroizing::new(plaintext.to_vec());
        key.seal_in_place_append_tag(nonce, Aad::empty(), &mut *buffer)
            .map_err(|_| anyhow!("AES-GCM seal failed"))?;
        let encoded = STANDARD_NO_PAD.encode(buffer.as_slice());
        Ok(format!("{}:{}", base64_nonce(nonce_bytes), encoded))
    }

    /// Keyed duplicate-detection token: hex(HMAC-SHA256(data_key, domain || plaintext)). Only an
    /// unlocked vault can produce or confirm one, so a protected row can be matched against a
    /// candidate copy without decrypting anything and without a reusable content hash. Domain
    /// separation keeps this token from ever being interchangeable with ciphertext.
    pub fn match_token(&self, plaintext: &[u8]) -> String {
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, self.data_key.as_ref());
        let mut context = ring::hmac::Context::with_key(&key);
        context.update(b"rustickers-match-v1\n");
        context.update(plaintext);
        let tag = context.sign();
        let mut out = String::with_capacity(64);
        for byte in tag.as_ref() {
            use std::fmt::Write;
            let _ = write!(&mut out, "{byte:02x}");
        }
        out
    }

    /// Inverse of [`seal`]. Returns the plaintext wrapped in a [`Zeroizing`] buffer; the caller
    /// is responsible for dropping the buffer (or moving it to a feature-gated scratch area).
    pub fn open(&self, sealed: &str) -> Result<Zeroizing<Vec<u8>>> {
        let (nonce_b64, payload_b64) = sealed
            .split_once(':')
            .ok_or_else(|| anyhow!("sealed payload is missing the nonce separator"))?;
        let nonce_bytes = STANDARD_NO_PAD
            .decode(nonce_b64.as_bytes())
            .map_err(|_| anyhow!("nonce base64 invalid"))?;
        if nonce_bytes.len() != NONCE_LEN {
            bail!("nonce length is wrong: {} bytes", nonce_bytes.len());
        }
        let mut payload = Zeroizing::new(
            STANDARD_NO_PAD
                .decode(payload_b64.as_bytes())
                .map_err(|_| anyhow!("ciphertext base64 invalid"))?,
        );
        let unbound = UnboundKey::new(&AES_256_GCM, self.data_key.as_ref())
            .map_err(|_| anyhow!("invalid AES-256-GCM key"))?;
        let key = LessSafeKey::new(unbound);
        let mut nonce_array = [0u8; NONCE_LEN];
        nonce_array.copy_from_slice(&nonce_bytes);
        let nonce = Nonce::assume_unique_for_key(nonce_array);
        let plaintext = key
            .open_in_place(nonce, Aad::empty(), &mut payload)
            .map_err(|_| anyhow!("AES-GCM open failed (wrong master password?)"))?;
        Ok(Zeroizing::new(plaintext.to_vec()))
    }
}

/// Construct a fresh envelope wrapping a randomly-generated 32-byte data key. Used during
/// `vault setup` to bootstrap the user. The data key stays in memory; only the envelope survives.
pub fn create_envelope(master_password: &str) -> Result<UnlockedVault> {
    let salt = random_bytes::<SALT_LEN>();
    let data_key = Zeroizing::new(random_bytes::<32>());
    let envelope = derive_and_seal(master_password, &salt, data_key.as_ref())?;
    Ok(UnlockedVault { envelope, data_key })
}

/// Re-derive the wrapping key from the master password and use it to unseal the data key.
pub fn unlock(envelope: &VaultEnvelope, master_password: &str) -> Result<UnlockedVault> {
    anyhow::ensure!(
        envelope.m_cost == ARGON2_M_COST_KIB
            && envelope.t_cost == ARGON2_T_COST
            && envelope.p_cost == ARGON2_P_COST,
        "unsupported vault KDF parameters"
    );
    let salt = STANDARD_NO_PAD
        .decode(envelope.salt.as_bytes())
        .map_err(|_| anyhow!("vault salt base64 invalid"))?;
    if salt.len() != SALT_LEN {
        bail!("vault salt has wrong length: {} bytes", salt.len());
    }
    let mut salt_array = [0u8; SALT_LEN];
    salt_array.copy_from_slice(&salt);

    let mut wrapping_key = Zeroizing::new([0u8; 32]);
    argon2id(
        master_password.as_bytes(),
        &salt_array,
        envelope.m_cost,
        envelope.t_cost,
        envelope.p_cost,
        wrapping_key.as_mut(),
    )
    .context("Argon2id derivation failed")?;

    let nonce_bytes = STANDARD_NO_PAD
        .decode(envelope.nonce.as_bytes())
        .map_err(|_| anyhow!("vault nonce base64 invalid"))?;
    if nonce_bytes.len() != NONCE_LEN {
        bail!("vault nonce length is wrong");
    }
    let mut nonce_array = [0u8; NONCE_LEN];
    nonce_array.copy_from_slice(&nonce_bytes);
    let nonce = Nonce::assume_unique_for_key(nonce_array);

    let key = UnboundKey::new(&AES_256_GCM, wrapping_key.as_ref())
        .map_err(|_| anyhow!("invalid wrapping key"))?;
    let mut payload = Zeroizing::new(
        STANDARD_NO_PAD
            .decode(envelope.ciphertext.as_bytes())
            .map_err(|_| anyhow!("vault ciphertext base64 invalid"))?,
    );
    let opened = LessSafeKey::new(key)
        .open_in_place(nonce, Aad::empty(), &mut payload)
        .map_err(|_| anyhow!("vault decryption failed (wrong master password?)"))?;
    let data_key_bytes = Zeroizing::new(opened.to_vec());
    if data_key_bytes.len() != 32 {
        bail!("vault decrypted data key has wrong length");
    }
    let mut array = Zeroizing::new([0u8; 32]);
    array.copy_from_slice(&data_key_bytes);
    Ok(UnlockedVault {
        envelope: envelope.clone(),
        data_key: array,
    })
}

/// Recompute the wrapping key; returns a new envelope that uses fresh nonce + salt. Used after
/// the user changes their master password.
pub fn rewrap(previous_envelope: UnlockedVault, new_password: &str) -> Result<UnlockedVault> {
    let salt = random_bytes::<SALT_LEN>();
    let envelope = derive_and_seal(new_password, &salt, previous_envelope.data_key.as_ref())?;
    Ok(UnlockedVault {
        envelope,
        data_key: previous_envelope.data_key.clone(),
    })
}

fn derive_and_seal(
    password: &str,
    salt: &[u8; SALT_LEN],
    data_key: &[u8],
) -> Result<VaultEnvelope> {
    let mut wrapping_key = Zeroizing::new([0u8; 32]);
    argon2id(
        password.as_bytes(),
        salt,
        ARGON2_M_COST_KIB,
        ARGON2_T_COST,
        ARGON2_P_COST,
        wrapping_key.as_mut(),
    )
    .context("Argon2id KDF failed")?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let mut ciphertext = Zeroizing::new(data_key.to_vec());
    let key = UnboundKey::new(&AES_256_GCM, wrapping_key.as_ref())
        .map_err(|_| anyhow!("invalid wrapping key"))?;
    LessSafeKey::new(key)
        .seal_in_place_append_tag(nonce, Aad::empty(), &mut *ciphertext)
        .map_err(|_| anyhow!("AES-GCM seal of the data key failed"))?;
    Ok(VaultEnvelope {
        salt: base64_nonce(*salt),
        nonce: base64_nonce(nonce_bytes),
        ciphertext: STANDARD_NO_PAD.encode(ciphertext.as_slice()),
        m_cost: ARGON2_M_COST_KIB,
        t_cost: ARGON2_T_COST,
        p_cost: ARGON2_P_COST,
    })
}

/// Randomly generate N bytes using the OS RNG. Wrap the call so callers don't have to import
/// `rand` directly.
fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    rand::thread_rng().fill_bytes(&mut out);
    out
}

fn base64_nonce<const N: usize>(bytes: [u8; N]) -> String {
    STANDARD_NO_PAD.encode(bytes.as_slice())
}

/// Argon2id wrapper around `argon2` crate. The master password stays in memory only as long as
/// the KDF is running; afterwards the only remaining copy is the temporary wrapping key.
fn argon2id(
    password: &[u8],
    salt: &[u8; SALT_LEN],
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
    out: &mut [u8],
) -> Result<()> {
    use argon2::{Algorithm, Argon2, Params, Version};
    let params = Params::new(m_cost, t_cost, p_cost, Some(out.len()))
        .map_err(|err| anyhow!("invalid Argon2 parameters: {err}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    argon
        .hash_password_into(password, salt.as_slice(), out)
        .map_err(|err| anyhow!("Argon2id hash failed: {err}"))?;
    Ok(())
}

/// Wipe the in-memory data key on drop.
impl Drop for UnlockedVault {
    fn drop(&mut self) {
        self.data_key.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_token_is_keyed_stable_and_distinct() {
        let vault = create_envelope("master").expect("create");
        let token = vault.match_token(b"secret text");
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        // Stable for the same key and content; different content never collides.
        assert_eq!(token, vault.match_token(b"secret text"));
        assert_ne!(token, vault.match_token(b"secret texu"));
        // And only this vault's key can reproduce it: a second vault proves the token is keyed,
        // not a plain content hash.
        let other = create_envelope("master").expect("create");
        assert_ne!(token, other.match_token(b"secret text"));
    }

    #[test]
    fn create_then_unlock_round_trips() {
        let vault = create_envelope("correct horse battery staple").expect("create");
        let envelope = vault.envelope().clone();

        // Dropping the original drops the data key.
        drop(vault);

        let unlocked = unlock(&envelope, "correct horse battery staple").expect("unlock");
        assert_eq!(unlocked.data_key.len(), 32);
    }

    #[test]
    fn wrong_password_fails_to_unlock() {
        let vault = create_envelope("hunter2").expect("create");
        let envelope = vault.envelope().clone();
        drop(vault);
        let err = unlock(&envelope, "hunter3").unwrap_err();
        assert!(format!("{err:#}").contains("wrong master password"));
    }

    #[test]
    fn sealing_and_opening_round_trips() {
        let vault = create_envelope("open sesame").expect("create");
        let sealed = vault.seal(b"hello clipboard").expect("seal");
        let plaintext = vault.open(&sealed).expect("open");
        assert_eq!(plaintext.as_slice(), b"hello clipboard");
    }

    #[test]
    fn unknown_prefix_is_rejected() {
        let result = VaultEnvelope::parse("OTHER_VERSION\n{\"salt\": \"aGVsbG8=\"}");
        assert!(result.is_err());
    }

    #[test]
    fn rewrap_changes_envelope_but_preserves_data_key() {
        let vault = create_envelope("old").expect("create");
        let data_key_copy = *vault.data_key;
        let rewrapped = rewrap(vault, "new").expect("rewrap");
        assert_eq!(rewrapped.data_key.as_ref(), &data_key_copy);

        let opened_after_rewrap = rewrapped.seal(b"payload").expect("seal after rewrap");
        let read = rewrapped
            .open(&opened_after_rewrap)
            .expect("open after rewrap");
        assert_eq!(read.as_slice(), b"payload");

        // Old password no longer accepts the new envelope.
        assert!(unlock(rewrapped.envelope(), "old").is_err());
    }
}

/// Validate structure before storing ciphertext. Authentication still requires the vault key.
pub fn validate_sealed(sealed: &str) -> Result<()> {
    let (nonce, payload) = sealed
        .split_once(':')
        .context("sealed payload missing nonce")?;
    let nonce = STANDARD_NO_PAD
        .decode(nonce)
        .context("invalid sealed nonce")?;
    let payload = Zeroizing::new(
        STANDARD_NO_PAD
            .decode(payload)
            .context("invalid sealed ciphertext")?,
    );
    anyhow::ensure!(
        nonce.len() == NONCE_LEN && payload.len() >= AES_256_GCM.tag_len(),
        "invalid sealed payload lengths"
    );
    Ok(())
}

#[cfg(test)]
mod safety_regressions {
    use super::*;
    #[test]
    fn malformed_envelope_diagnostics_do_not_echo_clipboard_content() {
        let error = VaultEnvelope::parse("private clipboard content").unwrap_err();
        assert!(!format!("{error:#}").contains("private clipboard content"));
    }
    #[test]
    fn untrusted_kdf_cost_is_rejected_before_allocating() {
        let envelope = VaultEnvelope {
            salt: base64_nonce([0u8; SALT_LEN]),
            nonce: String::new(),
            ciphertext: String::new(),
            m_cost: u32::MAX,
            t_cost: u32::MAX,
            p_cost: 2,
        };
        assert!(
            unlock(&envelope, "password")
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
    }
}
