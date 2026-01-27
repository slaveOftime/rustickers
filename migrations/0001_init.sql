-- sqlx migration: initial schema

CREATE TABLE IF NOT EXISTS stickers (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    title       TEXT NOT NULL,
    state       TEXT NOT NULL,
    left        INTEGER NOT NULL,
    top         INTEGER NOT NULL,
    width       INTEGER NOT NULL,
    height      INTEGER NOT NULL,
    color       TEXT NOT NULL,
    type        TEXT NOT NULL,
    content     TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_stickers_created_at ON stickers(created_at);
CREATE INDEX IF NOT EXISTS idx_stickers_updated_at ON stickers(updated_at);
CREATE INDEX IF NOT EXISTS idx_stickers_title      ON stickers(title);
