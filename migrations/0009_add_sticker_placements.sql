-- Remember one placement per monitor, plus the monitor the user deliberately chose.

ALTER TABLE stickers ADD COLUMN preferred_display_uuid TEXT;

CREATE TABLE IF NOT EXISTS sticker_placements (
    sticker_id    INTEGER NOT NULL,
    display_uuid  TEXT    NOT NULL,
    display_id    INTEGER,
    native_left   INTEGER NOT NULL,
    native_top    INTEGER NOT NULL,
    native_width  INTEGER NOT NULL,
    native_height INTEGER NOT NULL,
    scale_factor  REAL    NOT NULL,
    updated_at    INTEGER NOT NULL,
    PRIMARY KEY (sticker_id, display_uuid)
);

CREATE INDEX IF NOT EXISTS idx_sticker_placements_sticker ON sticker_placements(sticker_id);

-- Seed the first record from the single placement stickers used to remember.
INSERT OR IGNORE INTO sticker_placements (
    sticker_id, display_uuid, display_id,
    native_left, native_top, native_width, native_height,
    scale_factor, updated_at
)
SELECT
    id,
    display_uuid,
    display_id,
    native_left,
    native_top,
    native_width,
    native_height,
    CASE
        WHEN width > 0 AND native_width > 0 THEN CAST(native_width AS REAL) / width
        ELSE 1.0
    END,
    updated_at
FROM stickers
WHERE display_uuid IS NOT NULL
  AND native_left IS NOT NULL
  AND native_top IS NOT NULL
  AND native_width IS NOT NULL
  AND native_height IS NOT NULL;

UPDATE stickers SET preferred_display_uuid = display_uuid WHERE display_uuid IS NOT NULL;
