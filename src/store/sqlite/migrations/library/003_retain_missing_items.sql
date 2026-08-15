-- Marks an item whose book file is no longer present under any library path.
--
-- Without this the only way to reconcile a vanished file is to delete the item,
-- which cascades its annotations away: the reading record dies with the file.
-- With --retain-missing the item is flagged instead, so highlights, notes,
-- status and statistics survive, and the UI can say the file is gone rather
-- than offering a link that cannot be served.
--
-- Defaults to 0, so existing rows and the default configuration are unchanged.
ALTER TABLE library_items ADD COLUMN file_missing INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_library_items_file_missing
    ON library_items (file_missing);
