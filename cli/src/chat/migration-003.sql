ALTER TABLE delivery_attempts ADD COLUMN outcome_json TEXT;
ALTER TABLE exposures ADD COLUMN fetched_at TEXT;
ALTER TABLE events ADD COLUMN kind TEXT NOT NULL DEFAULT 'message';
ALTER TABLE events ADD COLUMN message_id TEXT REFERENCES messages(id);
UPDATE events SET message_id = json_extract(payload_json, '$.id');
CREATE TABLE delivery_observations (
  recipient_key TEXT PRIMARY KEY,
  epoch INTEGER NOT NULL,
  state TEXT NOT NULL
);
