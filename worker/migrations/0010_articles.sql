CREATE TABLE articles (
  id TEXT PRIMARY KEY,
  owner_id TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'staging' CHECK (state IN ('staging', 'published', 'abandoned')),
  idempotency_key TEXT NOT NULL,
  manifest_hash TEXT NOT NULL,
  title TEXT NOT NULL,
  summary TEXT NOT NULL,
  name TEXT NOT NULL,
  source_host TEXT NOT NULL,
  source_project TEXT NOT NULL,
  silent INTEGER NOT NULL CHECK (silent IN (0, 1)),
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  read_at TEXT,
  version INTEGER NOT NULL DEFAULT 0 CHECK (version >= 0),
  sanitized_html_key TEXT NOT NULL UNIQUE,
  cleanup_after TEXT NOT NULL,
  cleanup_token TEXT,
  UNIQUE (owner_id, idempotency_key)
);
CREATE INDEX articles_published_order ON articles(owner_id, state, created_at DESC, id DESC);
CREATE INDEX articles_cleanup ON articles(state, cleanup_after);

CREATE TABLE article_assets (
  article_id TEXT NOT NULL REFERENCES articles(id),
  asset_index INTEGER NOT NULL CHECK (asset_index >= 0 AND asset_index <= 50),
  path TEXT NOT NULL,
  media_type TEXT NOT NULL,
  size INTEGER NOT NULL CHECK (size >= 0 AND size <= 20971520),
  sha256 TEXT NOT NULL,
  disposition TEXT NOT NULL CHECK (disposition IN ('inline', 'attachment')),
  object_key TEXT NOT NULL UNIQUE,
  complete INTEGER NOT NULL DEFAULT 0 CHECK (complete IN (0, 1)),
  PRIMARY KEY (article_id, asset_index),
  UNIQUE (article_id, path)
);

-- Task 2 inserts the publication job in the same D1 batch as the state transition.
-- Task 6 claims and delivers it. A retry cannot create a second job.
CREATE TABLE article_notification_jobs (
  article_id TEXT PRIMARY KEY REFERENCES articles(id),
  state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'sending', 'delivered')),
  attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  next_attempt_at TEXT NOT NULL,
  lease_until TEXT,
  lease_token TEXT,
  delivered_at TEXT
);
CREATE INDEX article_notification_due ON article_notification_jobs(state, next_attempt_at);
