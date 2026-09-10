-- A bookmark snapshots the message text: sessions are archived and deleted, so
-- a pure (session_id, seq) reference would silently empty. `ON DELETE SET NULL`
-- keeps the bookmark when its source session goes away; `session_name` is
-- denormalised so a dead-link bookmark still says where it came from.
CREATE TABLE IF NOT EXISTS bookmarks (
  id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  session_id   TEXT NULL REFERENCES sessions(id) ON DELETE SET NULL,
  seq          BIGINT NULL,
  message_id   TEXT NULL,
  title        TEXT NOT NULL,
  body         TEXT NOT NULL,
  role         TEXT NOT NULL,
  session_name TEXT NULL,
  note         TEXT NULL,
  message_ts   TIMESTAMPTZ NOT NULL,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS bookmarks_user_created_idx
  ON bookmarks (user_id, created_at DESC);

CREATE INDEX IF NOT EXISTS bookmarks_session_seq_idx
  ON bookmarks (session_id, seq);
