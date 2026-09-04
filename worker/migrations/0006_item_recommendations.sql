ALTER TABLE items ADD COLUMN recommendation TEXT;
ALTER TABLE items ADD COLUMN recommended_choice TEXT;
ALTER TABLE items ADD COLUMN dedupe_key TEXT;
CREATE INDEX items_open_dedupe ON items(dedupe_key, created_at DESC)
  WHERE status = 'open' AND dedupe_key IS NOT NULL;
