-- sqlx migration: add LRU cache table for selection-to-command feature

CREATE TABLE IF NOT EXISTS selection_lru (
    sticker_id INTEGER PRIMARY KEY,
    last_used_at INTEGER NOT NULL,
    FOREIGN KEY (sticker_id) REFERENCES stickers(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_selection_lru_last_used ON selection_lru(last_used_at);