-- sqlx migration: add top_most flag to stickers

ALTER TABLE stickers
ADD COLUMN top_most INTEGER NOT NULL DEFAULT 0;
