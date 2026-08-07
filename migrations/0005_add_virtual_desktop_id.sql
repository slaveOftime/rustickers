-- Remember the Windows virtual desktop containing each sticker.
ALTER TABLE stickers ADD COLUMN virtual_desktop_id TEXT;
