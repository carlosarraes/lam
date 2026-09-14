CREATE TABLE sessions (
  machine TEXT NOT NULL,
  incarnation TEXT NOT NULL,
  project TEXT NOT NULL,
  name TEXT NOT NULL,
  client TEXT NOT NULL,
  native_id TEXT NOT NULL,
  process_start TEXT NOT NULL,
  eligible INTEGER NOT NULL CHECK (eligible IN (0, 1)),
  connected INTEGER NOT NULL CHECK (connected IN (0, 1)),
  ended INTEGER NOT NULL CHECK (ended IN (0, 1)),
  PRIMARY KEY(machine, incarnation),
  UNIQUE(machine, client, native_id, process_start),
  CHECK (ended = 0 OR (eligible = 0 AND connected = 0))
);

CREATE INDEX sessions_project ON sessions(project);
