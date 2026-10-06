-- Migration 0011: master-password vault table.
--
-- Persistent only when the user configures a master password. `vault_envelope` is the
-- Argon2id-wrapped data key, exactly the format produced by `VaultEnvelope::serialize`. The
-- `vault_active_until` lets the user pick how long an unlocked vault stays open in idle; it is
-- a UNIX timestamp in milliseconds.
--
-- We keep the rest of the storage untouched: protected row encryption already lived in the
-- pre-existing schema; now we have a stable place to anchor the master.

CREATE TABLE IF NOT EXISTS clipboard_vault (
    id                       INTEGER PRIMARY KEY CHECK (id = 1),
    envelope                 TEXT NOT NULL,
    active_until_unix_ms     INTEGER NOT NULL DEFAULT 0
);

INSERT OR IGNORE INTO clipboard_vault (id, envelope, active_until_unix_ms)
VALUES (1, '', 0);
