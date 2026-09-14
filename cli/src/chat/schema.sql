CREATE TABLE metadata (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  sender_key TEXT NOT NULL,
  sender_seq INTEGER NOT NULL,
  idempotency_key TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  UNIQUE(sender_key, idempotency_key),
  UNIQUE(sender_key, sender_seq)
);

CREATE TABLE recipients (
  message_id TEXT NOT NULL REFERENCES messages(id),
  target_key TEXT NOT NULL,
  PRIMARY KEY(message_id, target_key)
);

CREATE TABLE events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  project TEXT NOT NULL,
  project_seq INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  UNIQUE(project, project_seq)
);

CREATE TABLE inbox_entries (
  recipient_key TEXT NOT NULL,
  inbox_seq INTEGER NOT NULL,
  message_id TEXT NOT NULL REFERENCES messages(id),
  PRIMARY KEY(recipient_key, inbox_seq),
  UNIQUE(recipient_key, message_id)
);

CREATE TABLE delivery_receipts (
  message_id TEXT NOT NULL,
  target_key TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'queued',
  evidence_json TEXT,
  updated_at TEXT,
  PRIMARY KEY(message_id, target_key),
  FOREIGN KEY(message_id, target_key) REFERENCES recipients(message_id, target_key)
);

CREATE TABLE exposures (
  message_id TEXT NOT NULL,
  target_key TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'unseen',
  updated_at TEXT,
  PRIMARY KEY(message_id, target_key),
  FOREIGN KEY(message_id, target_key) REFERENCES recipients(message_id, target_key)
);

CREATE TABLE delivery_attempts (
  id TEXT PRIMARY KEY,
  recipient_key TEXT NOT NULL,
  state TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  completed_at TEXT
);

CREATE TABLE delivery_attempt_messages (
  attempt_id TEXT NOT NULL REFERENCES delivery_attempts(id),
  ordinal INTEGER NOT NULL,
  message_id TEXT NOT NULL REFERENCES messages(id),
  PRIMARY KEY(attempt_id, ordinal),
  UNIQUE(attempt_id, message_id)
);

CREATE INDEX inbox_entries_message ON inbox_entries(message_id);
CREATE INDEX events_project_replay ON events(project, project_seq);
CREATE INDEX delivery_attempts_recipient_state ON delivery_attempts(recipient_key, state);
CREATE UNIQUE INDEX delivery_attempts_one_active
  ON delivery_attempts(recipient_key) WHERE state = 'submitting';
