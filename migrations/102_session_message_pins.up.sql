CREATE TABLE IF NOT EXISTS session_message_pins (
  id         BIGSERIAL PRIMARY KEY,
  user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  seq        BIGINT NOT NULL,
  message_id TEXT,
  note       TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (user_id, session_id, seq)
);

CREATE INDEX IF NOT EXISTS session_message_pins_session_idx
  ON session_message_pins (session_id, user_id, seq);
