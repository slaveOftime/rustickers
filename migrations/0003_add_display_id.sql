-- sqlx migration: add display_id to stickers for multi-monitor support
ALTER TABLE stickers ADD COLUMN display_id INTEGER;
