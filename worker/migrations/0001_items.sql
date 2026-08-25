CREATE TABLE items (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  body TEXT NOT NULL DEFAULT '',
  source_host TEXT NOT NULL DEFAULT '',
  source_project TEXT NOT NULL DEFAULT '',
  priority TEXT NOT NULL DEFAULT 'normal',
  choices TEXT NOT NULL DEFAULT '[]',
  status TEXT NOT NULL DEFAULT 'open',
  response_choice TEXT,
  response_text TEXT,
  response_by TEXT,
  created_at TEXT NOT NULL,
  resolved_at TEXT
);
CREATE INDEX items_status_created ON items(status, created_at);
