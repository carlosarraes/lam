CREATE TABLE peer_outbox (
  project TEXT NOT NULL,
  origin_seq INTEGER NOT NULL,
  event_id TEXT NOT NULL UNIQUE,
  payload_json TEXT NOT NULL,
  PRIMARY KEY(project, origin_seq)
);

CREATE TABLE imported_events (
  event_id TEXT PRIMARY KEY,
  origin TEXT NOT NULL,
  project TEXT NOT NULL,
  origin_seq INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  UNIQUE(origin, project, origin_seq)
);

CREATE TABLE peer_cursors (
  peer TEXT NOT NULL,
  project TEXT NOT NULL,
  received_through INTEGER NOT NULL DEFAULT 0,
  acknowledged_through INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(peer, project)
);

CREATE TABLE remote_sessions (
  machine TEXT NOT NULL,
  incarnation TEXT NOT NULL,
  project TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  epoch INTEGER NOT NULL,
  PRIMARY KEY(machine, incarnation)
);

CREATE INDEX imported_events_replay ON imported_events(origin, project, origin_seq);
CREATE INDEX remote_sessions_project ON remote_sessions(project);
