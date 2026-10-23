-- Sessions a human has open in a client, per server replica. Each replica
-- re-stamps the sessions its sockets subscribe to; a row whose stamp lapses
-- means nobody is looking any more.
CREATE TABLE session_viewers (
    session_id TEXT        NOT NULL,
    pod        TEXT        NOT NULL,
    seen_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (session_id, pod)
);
