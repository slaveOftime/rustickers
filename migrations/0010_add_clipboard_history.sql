-- Clipboard history tables.
--
-- Items are stored as ordinary text or as a protected envelope, never both. The
-- retention reasons are independent flags so a protected item that was edited
-- keeps both reasons after unlocks.
--
-- `plaintext_digest` is a SHA-256 of the lowercase-trimmed content. It is used
-- only for ordinary dedupe and never stored for protected rows. It is not a
-- secret and does not leak content: it lets us recognise repeated copies of
-- ordinary text without storing a separate hash table.
--
-- `protected_match_token` is a per-vault HMAC tag over the protected plaintext,
-- computed only when a vault is configured and an item is unlocked during
-- capture. The vault key is required to recompute it, so an attacker without
-- the master password cannot use it to detect protected duplicates. While the
-- vault is locked the field is NULL and ordinary captures of protected text
-- cannot be deduplicated against it.

CREATE TABLE IF NOT EXISTS clipboard_items (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    content               TEXT NOT NULL,
    content_bytes         INTEGER NOT NULL,
    plain_preview         TEXT,
    plaintext_digest      TEXT,
    protected_match_token TEXT,
    protected_envelope    TEXT,
    public_label          TEXT,
    retention_explicit    INTEGER NOT NULL DEFAULT 0,
    retention_edited      INTEGER NOT NULL DEFAULT 0,
    retention_protected   INTEGER NOT NULL DEFAULT 0,
    locked                INTEGER NOT NULL DEFAULT 0,
    created_at            INTEGER NOT NULL,
    last_copied_at        INTEGER NOT NULL,
    last_used_at          INTEGER,
    last_edited_at        INTEGER,
    byte_size             INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_clipboard_recency
    ON clipboard_items (MAX(COALESCE(last_copied_at, 0), COALESCE(last_used_at, 0), COALESCE(last_edited_at, 0), created_at) DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_clipboard_plain_digest
    ON clipboard_items (plaintext_digest)
    WHERE plaintext_digest IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_clipboard_protected_match
    ON clipboard_items (protected_match_token)
    WHERE protected_match_token IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_clipboard_retained
    ON clipboard_items (retention_explicit, retention_edited, retention_protected);

CREATE TABLE IF NOT EXISTS clipboard_settings (
    key                   TEXT PRIMARY KEY,
    value                 TEXT NOT NULL
);

INSERT OR IGNORE INTO clipboard_settings (key, value) VALUES
    ('normal_count_limit', '500'),
    ('normal_byte_limit',  '20971520'),
    ('capture_enabled',    '0'),
    ('schema_version',     '1');
