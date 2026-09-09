CREATE TABLE IF NOT EXISTS session_file_links (
  session_id    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  path          TEXT NOT NULL,
  first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (session_id, path)
);

CREATE TABLE IF NOT EXISTS session_blob_links (
  session_id    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  hash          TEXT NOT NULL,
  first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (session_id, hash)
);
