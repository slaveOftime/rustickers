//! Persistent master-password vault plus in-memory unlock state.
//!
//! This is the controller GUI and CLI callers reach for when deciding whether a clipboard
//! row can be exposed. It is **not** the vault itself; the wrapping/unwrapping math lives in
//! [`super::vault`]. The controller owns:
//!
//! 1. The on-disk envelope in the `clipboard_vault` table.
//! 2. The current `UnlockedVault` kept in memory while the user is at a recent paste target.
//! 3. The scoped unlock deadline used by every cryptographic operation.
//!
//! The controller never logs the password or the data key. Failed unlocks are counted so the
//! caller can back off when needed (clipboard manager out of scope; the auto-clear happens after
//! `active_until_unix_ms`).

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use anyhow::{Result, anyhow};
use zeroize::Zeroizing;

use super::storage::ClipboardHistoryStore;
use super::vault::{UnlockedVault, VaultEnvelope, create_envelope, rewrap, unlock};

/// Default idle lifetime: 5 minutes from the last unlock, in milliseconds. Pastes of protected
/// rows are restricted to this window.
pub const DEFAULT_ACTIVE_LIFETIME_MS: i64 = 5 * 60 * 1000;

/// Manages a persistent master-password vault.
///
/// All really sensitive material flows through [`Zeroizing`] buffers; the controller does not own
/// the data key beyond the scoped unlock lifetime.
pub struct VaultController {
    store: Arc<dyn ClipboardHistoryStore>,
    operations: futures::lock::Mutex<()>,
    generation: AtomicI64,
    /// Unix-ms deadline past which the next paste target sees the vault as locked.
    active_until_unix_ms: AtomicI64,
    /// Cached unlocked vault; cleared on lock.
    unlocked: std::sync::Mutex<Option<UnlockedVault>>,
    /// Counter for failed unlock attempts to power back-off logic. Public for the GUI modal.
    failed_attempts: AtomicI64,
}

/// Ciphertext and the exact cached vault configuration used to seal it. No plaintext or key
/// is exported. Pass both fields to generation-bound storage in the same operation.
pub struct SealedClipboardPayload {
    pub ciphertext: String,
    pub vault_envelope: String,
}

impl std::fmt::Debug for SealedClipboardPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedClipboardPayload")
            .field("ciphertext", &"[redacted]")
            .field("vault_envelope", &"[redacted]")
            .finish()
    }
}
/// Permanent vault configuration; either we have one or we don't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultStatus {
    pub has_master_password: bool,
    pub failed_attempts: i64,
}

/// Lightweight snapshot of vault state, used by the GUI modal to decide between Create and Unlock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultState {
    /// No envelope yet; the user must create one.
    Unconfigured,
    /// Envelope exists but is locked.
    Locked,
    /// Envelope is unlocked and a data key is available.
    Unlocked { failed_attempts: i64 },
}

impl VaultState {
    pub fn needs_password(&self) -> bool {
        matches!(self, VaultState::Locked)
    }
}

impl VaultController {
    pub fn new(store: Arc<dyn ClipboardHistoryStore>) -> Self {
        Self {
            store,
            operations: futures::lock::Mutex::new(()),
            generation: AtomicI64::new(0),
            active_until_unix_ms: AtomicI64::new(0),
            unlocked: std::sync::Mutex::new(None),
            failed_attempts: AtomicI64::new(0),
        }
    }

    /// Read the on-disk envelope. Returns `Ok(None)` if no master password has been configured
    /// yet.
    pub async fn envelope(&self) -> Result<Option<VaultEnvelope>> {
        match self.store.read_vault_envelope().await? {
            Some(text) => Ok(Some(VaultEnvelope::parse(&text)?)),
            None => Ok(None),
        }
    }

    /// First-time bootstrap. Storage refuses replacement of an existing vault. Never orphan
    /// existing protected items by generating a replacement data key.
    pub async fn create(&self, password: &str) -> Result<()> {
        let _operation = self.operations.lock().await;
        let unlocked = create_envelope(password)?;
        let envelope = unlocked.envelope().clone();
        let serialized = VaultEnvelope::serialize_string(&envelope)?;
        self.store.write_vault_envelope(&serialized, 0).await?;
        self.lock();
        Ok(())
    }

    /// Attempt to unlock with the user-supplied password. Returns `Ok(true)` on success and
    /// `Ok(false)` on a wrong password (so the GUI modal can decide between back-off and a
    /// "wrong password" message).
    pub async fn unlock(&self, password: &str) -> Result<bool> {
        let _operation = self.operations.lock().await;
        let generation = self.generation.load(Ordering::SeqCst);
        let Some(text) = self.store.read_vault_envelope().await? else {
            return Err(anyhow!(
                "No master password is configured yet; call create() first"
            ));
        };
        let envelope = VaultEnvelope::parse(&text)?;
        match unlock(&envelope, password) {
            Ok(unlocked) => {
                let deadline = crate::utils::time::now_unix_millis() + DEFAULT_ACTIVE_LIFETIME_MS;
                let mut guard = self
                    .unlocked
                    .lock()
                    .map_err(|_| anyhow!("vault mutex poisoned"))?;
                anyhow::ensure!(
                    generation == self.generation.load(Ordering::SeqCst),
                    "vault locked during unlock; try again"
                );
                self.active_until_unix_ms.store(deadline, Ordering::SeqCst);
                *guard = Some(unlocked);
                self.failed_attempts.store(0, Ordering::SeqCst);
                Ok(true)
            }
            Err(_) => {
                let _ = self.failed_attempts.fetch_add(1, Ordering::SeqCst);
                Ok(false)
            }
        }
    }

    /// Re-wrap the data key using a new password. Existing protected-row ciphertexts stay
    /// decryptable because they are sealed with the data key, not the password directly.
    pub async fn rewrap(&self, password: &str) -> Result<()> {
        let _operation = self.operations.lock().await;
        let current = {
            let guard = self
                .unlocked
                .lock()
                .map_err(|_| anyhow!("vault mutex poisoned"))?;
            guard
                .as_ref()
                .ok_or_else(|| anyhow!("rewrap requires the vault to be unlocked"))?
                .clone()
        };
        anyhow::ensure!(self.is_unlocked(), "rewrap requires an unexpired unlock");
        let expected = current.envelope().serialize()?;
        let rewritten = rewrap(current, password)?;
        let serialized = VaultEnvelope::serialize_string(rewritten.envelope())?;
        self.store
            .compare_exchange_vault_envelope(
                &expected,
                &serialized,
                self.active_until_unix_ms.load(Ordering::SeqCst),
            )
            .await?;
        self.lock();
        Ok(())
    }

    /// Lock the vault immediately; subsequent calls that ask whether it is `is_unlocked` will
    /// return false.
    pub fn lock(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut guard) = self.unlocked.lock() {
            *guard = None;
        }
        self.active_until_unix_ms.store(0, Ordering::SeqCst);
    }

    /// True only when the dedicated deadline has not elapsed and the in-memory key is still
    /// cached. The caller should normally rely on [`paste_window`] instead.
    pub fn is_unlocked(&self) -> bool {
        let Ok(mut guard) = self.unlocked.lock() else {
            return false;
        };
        if crate::utils::time::now_unix_millis() >= self.active_until_unix_ms.load(Ordering::SeqCst)
        {
            *guard = None;
            return false;
        }
        guard.is_some()
    }
    /// Comment-friendly alias for the GUI/CLI who want to spell out the relationship.
    pub fn paste_window(&self) -> bool {
        self.is_unlocked()
    }

    /// Number of consecutive failed unlock attempts since the last successful unlock. The GUI
    /// uses this to suggest a back-off after a certain number of failed attempts.
    pub fn failed_attempts(&self) -> i64 {
        self.failed_attempts.load(Ordering::SeqCst)
    }

    /// Single-flight snapshot that drives the GUI modal's "create vs. unlock" decision and lets
    /// the editor refuse lock toggles when the vault is locked.
    pub async fn snapshot(&self) -> VaultState {
        match self.store.read_vault_envelope().await {
            Ok(Some(text)) if !text.is_empty() => {
                if self.is_unlocked() {
                    VaultState::Unlocked {
                        failed_attempts: self.failed_attempts(),
                    }
                } else {
                    VaultState::Locked
                }
            }
            Ok(None) => VaultState::Unconfigured,
            _ => VaultState::Locked,
        }
    }

    /// Remove the master password only after all encrypted history has been removed.
    /// Storage checks this in a write transaction, including concurrent protected inserts.
    /// The vault key is preserved if disabling would strand protected data.
    pub async fn disable(&self) -> Result<()> {
        let _operation = self.operations.lock().await;
        self.store.clear_vault_envelope().await?;
        self.lock();
        Ok(())
    }

    pub fn seal(&self, plaintext: &[u8]) -> Result<Option<String>> {
        let mut guard = self
            .unlocked
            .lock()
            .map_err(|_| anyhow!("vault mutex poisoned"))?;
        if crate::utils::time::now_unix_millis() >= self.active_until_unix_ms.load(Ordering::SeqCst)
        {
            *guard = None;
            return Ok(None);
        }
        let Some(unlocked) = guard.as_ref() else {
            return Ok(None);
        };
        Ok(Some(unlocked.seal(plaintext)?))
    }

    /// Capture ciphertext and its vault generation under the same key lock. Never pair `seal`
    /// with a later `envelope()` read: another controller may reset the vault between those calls.
    pub fn seal_for_storage(&self, plaintext: &[u8]) -> Result<Option<SealedClipboardPayload>> {
        let mut guard = self
            .unlocked
            .lock()
            .map_err(|_| anyhow!("vault mutex poisoned"))?;
        if crate::utils::time::now_unix_millis() >= self.active_until_unix_ms.load(Ordering::SeqCst)
        {
            *guard = None;
            return Ok(None);
        }
        let Some(unlocked) = guard.as_ref() else {
            return Ok(None);
        };
        Ok(Some(SealedClipboardPayload {
            ciphertext: unlocked.seal(plaintext)?,
            vault_envelope: unlocked.envelope().serialize()?,
        }))
    }

    /// Keyed match token for authorized recapture matching ([`UnlockedVault::match_token`] /
    /// [`SqliteClipboardStore::upsert_ordinary`]). Returns `Ok(None)` when locked or expired,
    /// exactly like [`seal`]: an unattended caller treats the absence of a token as
    /// "not authorized" and fails closed.
    ///
    /// [`SqliteClipboardStore::upsert_ordinary`]: crate::clipboard::storage::SqliteClipboardStore::upsert_ordinary
    pub fn match_token(&self, plaintext: &[u8]) -> Result<Option<String>> {
        let mut guard = self
            .unlocked
            .lock()
            .map_err(|_| anyhow!("vault mutex poisoned"))?;
        if crate::utils::time::now_unix_millis() >= self.active_until_unix_ms.load(Ordering::SeqCst)
        {
            *guard = None;
            return Ok(None);
        }
        let Some(unlocked) = guard.as_ref() else {
            return Ok(None);
        };
        Ok(Some(unlocked.match_token(plaintext)))
    }

    /// Inverse of [`seal`]. Returns `Ok(None)` when the vault is currently locked so callers can
    /// distinguish *sealed-but-unavailable* from *sealed-and-decryptable*.
    pub fn open(&self, envelope: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let mut guard = self
            .unlocked
            .lock()
            .map_err(|_| anyhow!("vault mutex poisoned"))?;
        if crate::utils::time::now_unix_millis() >= self.active_until_unix_ms.load(Ordering::SeqCst)
        {
            *guard = None;
            return Ok(None);
        }
        let Some(unlocked) = guard.as_ref() else {
            return Ok(None);
        };
        Ok(Some(unlocked.open(envelope)?))
    }
}

/// Bundle the vault controller with the chooser's needs. The chooser only ever checks
/// `is_unlocked`; we expose it through this lightweight struct so we don't have to plumb the
/// storage trait around.
pub struct VaultHandle {
    pub controller: Arc<VaultController>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::storage::SqliteClipboardStore;

    static PROCESS_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    async fn setup() -> (
        Arc<dyn ClipboardHistoryStore>,
        std::path::PathBuf,
        Arc<VaultController>,
    ) {
        let path = std::env::temp_dir().join(format!(
            "rustickers-vault-controller-{}-{}.db",
            crate::utils::time::now_unix_millis(),
            PROCESS_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let store: Arc<dyn ClipboardHistoryStore> =
            Arc::new(SqliteClipboardStore::open(&path).await.unwrap());
        let controller = Arc::new(VaultController::new(store.clone()));
        (store, path, controller)
    }
    #[test]
    fn create_unlock_lock_round_trip() {
        use futures::executor::block_on;
        let (_store, path, controller) = block_on(setup());

        block_on(controller.create("hunter2-correct-horse")).expect("create vault");
        assert!(block_on(controller.envelope()).unwrap().is_some());
        assert!(
            !controller.is_unlocked(),
            "vault should not be unlocked right after create"
        );

        let good = block_on(controller.unlock("hunter2-correct-horse")).expect("unlock ok");
        assert!(good, "correct password should unlock");
        assert!(controller.is_unlocked());

        controller.lock();
        assert!(!controller.is_unlocked());

        let bad = block_on(controller.unlock("nope")).expect("unlock ok");
        assert!(!bad, "wrong password must not unlock");
        assert_eq!(controller.failed_attempts(), 1);

        block_on(controller.disable()).expect("disable vault");
        assert!(block_on(controller.envelope()).unwrap().is_none());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn seal_and_open_consume_vault_state() {
        use futures::executor::block_on;
        let (_store, path, controller) = block_on(setup());
        block_on(controller.create("hunter2")).expect("create");
        assert!(matches!(controller.seal(b"hello"), Ok(None)));
        assert!(block_on(controller.unlock("hunter2")).unwrap());
        let envelope = controller.seal(b"hello").unwrap().expect("seal ok");
        let plain = controller.open(&envelope).unwrap().expect("open ok");
        assert_eq!(&*plain, b"hello");
        controller.lock();
        assert!(controller.open(&envelope).unwrap().is_none());
        block_on(controller.disable()).unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn snapshot_drives_create_vs_unlock() {
        use futures::executor::block_on;
        let (_store, path, controller) = block_on(setup());
        // Fresh store: Unconfigured.
        match block_on(controller.snapshot()) {
            crate::clipboard::vault_controller::VaultState::Unconfigured => {}
            other => panic!("expected unconfigured, got {other:?}"),
        }
        block_on(controller.create("hunter2")).unwrap();
        match block_on(controller.snapshot()) {
            crate::clipboard::vault_controller::VaultState::Locked => {}
            other => panic!("expected locked after create, got {other:?}"),
        }
        assert!(block_on(controller.unlock("hunter2")).unwrap());
        match block_on(controller.snapshot()) {
            crate::clipboard::vault_controller::VaultState::Unlocked { .. } => {}
            other => panic!("expected unlocked, got {other:?}"),
        }
        block_on(controller.disable()).unwrap();
        match block_on(controller.snapshot()) {
            crate::clipboard::vault_controller::VaultState::Unconfigured => {}
            other => panic!("expected unconfigured after disable, got {other:?}"),
        }
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod safety_regressions {
    use super::*;
    use crate::clipboard::storage::SqliteClipboardStore;
    use futures::executor::block_on;

    #[test]
    fn expired_unlock_cannot_open_seal_or_rewrap() {
        let tmp = tempfile::tempdir().unwrap();
        let store =
            Arc::new(block_on(SqliteClipboardStore::open(tmp.path().join("clip.db"))).unwrap());
        let controller = VaultController::new(store);
        block_on(controller.create("password")).unwrap();
        assert!(block_on(controller.unlock("password")).unwrap());
        let sealed = controller.seal(b"secret").unwrap().unwrap();
        controller.active_until_unix_ms.store(1, Ordering::SeqCst);
        assert!(!controller.is_unlocked());
        assert!(controller.open(&sealed).unwrap().is_none());
        assert!(controller.seal(b"secret").unwrap().is_none());
        assert!(block_on(controller.rewrap("new")).is_err());
    }
}
