-- Content-addressable dedup across the captures and stickers tables.
--
-- `text_digest` stores the SHA-256 of the underlying plaintext bytes for
-- markdown stickers, computed at the moment the row is created, edited, or
-- re-encrypted. Application startup backfills legacy plaintext rows; legacy
-- encrypted rows remain empty until an authorized edit, unlock, or password
-- rotation provides plaintext. This SQL migration never needs the master password.
--
-- Captures already deduplicate against `clipboard_captures.text_digest`. The
-- new column lets the capture step deduplicate against saved text too: when
-- clipboard text matches an already-saved (plaintext or locked) sticker by
-- digest, the capture is dropped and only the sticker's recency metadata is
-- bumped. That keeps duplicate plaintext from lingering next to a sealed copy
-- and surfaces the saved entry at the top of the recency-sorted list.
--
-- `text_digest` is a content fingerprint, not a confidentiality mechanism.
-- Database readers can correlate equal encrypted content and test guessed
-- plaintext (especially low-entropy secrets) against its SHA-256. Do not expose
-- these fingerprints in redacted snapshots or logs. Existing
-- `text_*` recency columns stay exclusive to stickers so window/chrome writes
-- do not advance text activity.

ALTER TABLE stickers ADD COLUMN text_digest TEXT NOT NULL DEFAULT '';

CREATE INDEX IF NOT EXISTS idx_stickers_text_digest
    ON stickers (text_digest)
    WHERE text_digest != '';
