ALTER TABLE items ADD COLUMN pending_at TEXT;
ALTER TABLE items ADD COLUMN external_key TEXT;
CREATE UNIQUE INDEX items_external_key_open ON items(external_key) WHERE status='open' AND external_key IS NOT NULL;
