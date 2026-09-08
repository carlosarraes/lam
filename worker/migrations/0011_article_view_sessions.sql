CREATE TABLE article_view_sessions (
  bootstrap_hash TEXT PRIMARY KEY,
  article_id TEXT NOT NULL REFERENCES articles(id),
  credential_hash TEXT NOT NULL,
  device_id TEXT,
  bootstrap_expires_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  session_hash TEXT UNIQUE,
  delivered_version INTEGER
);
CREATE INDEX article_view_sessions_expiry ON article_view_sessions(expires_at);
