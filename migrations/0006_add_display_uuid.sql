-- Stable physical-monitor identifier used across application restarts.
ALTER TABLE stickers ADD COLUMN display_uuid TEXT;
