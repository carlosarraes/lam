ALTER TABLE items ADD COLUMN kind TEXT NOT NULL DEFAULT 'request'
  CHECK (kind IN ('request', 'fyi'));
ALTER TABLE items ADD COLUMN seen_at TEXT;
