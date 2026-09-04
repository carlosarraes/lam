CREATE TABLE pairing_sessions (
  id TEXT PRIMARY KEY,
  secret_hash TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  consumed_at TEXT,
  device_id TEXT REFERENCES devices(id)
);

CREATE INDEX pairing_sessions_unconsumed_expiry
  ON pairing_sessions(expires_at)
  WHERE consumed_at IS NULL;
CREATE INDEX pairing_sessions_device_id ON pairing_sessions(device_id);
