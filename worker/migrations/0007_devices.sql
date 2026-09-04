CREATE TABLE devices (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  credential_hash TEXT NOT NULL UNIQUE,
  fcm_token TEXT,
  app_version TEXT NOT NULL,
  android_version TEXT NOT NULL,
  created_at TEXT NOT NULL,
  last_seen_at TEXT,
  revoked_at TEXT
);

CREATE INDEX devices_revoked_at ON devices(revoked_at);
CREATE INDEX devices_fcm_token ON devices(fcm_token);
