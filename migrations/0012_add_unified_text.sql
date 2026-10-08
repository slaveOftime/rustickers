-- Text recency is independent of window/chrome changes.
ALTER TABLE stickers ADD COLUMN text_modified_at INTEGER;
ALTER TABLE stickers ADD COLUMN text_last_used_at INTEGER;
ALTER TABLE stickers ADD COLUMN text_revision INTEGER NOT NULL DEFAULT 0;
UPDATE stickers SET text_modified_at = updated_at WHERE type = 'markdown';

-- New captures have no retention reasons, public labels, or encrypted payloads.
-- The legacy table remains until desktop and CLI adapters are cut over together.
CREATE TABLE clipboard_captures (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    text TEXT NOT NULL CHECK(length(text) > 0),
    text_digest TEXT NOT NULL,
    byte_size INTEGER NOT NULL CHECK(byte_size >= 0),
    created_at INTEGER NOT NULL,
    last_copied_at INTEGER NOT NULL,
    last_used_at INTEGER
);
CREATE INDEX clipboard_captures_digest ON clipboard_captures(text_digest);
CREATE INDEX clipboard_captures_recency ON clipboard_captures(
    MAX(created_at, last_copied_at, COALESCE(last_used_at, 0)) DESC, id DESC
);
CREATE TABLE text_master_key (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    configuration TEXT NOT NULL
);
CREATE TABLE text_imports (
    source TEXT PRIMARY KEY,
    imported_at INTEGER NOT NULL,
    imported_count INTEGER NOT NULL,
    skipped_count INTEGER NOT NULL
);
