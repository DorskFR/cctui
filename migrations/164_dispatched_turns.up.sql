-- A reply is not idempotent: a client that never saw its ack resends the same
-- turn, and without a record the agent is handed the prompt once per attempt.
-- The claim lives in the DB rather than in a process map because the
-- deployment runs several replicas with no affinity: a reconnecting client
-- lands on whichever one the ingress picks.
--
-- `command_id` is NULL while an attempt is in flight and set once the dispatch
-- succeeded; a refused dispatch deletes its row so the retry can try again. A
-- stale NULL row (the replica died mid-dispatch) is reclaimable after
-- `claimed_at` ages out, so a crash cannot wedge a session's turn forever.
CREATE TABLE dispatched_turns (
    session_id TEXT        NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_id    UUID        NOT NULL,
    command_id UUID,
    claimed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (session_id, turn_id)
);

CREATE INDEX dispatched_turns_claimed_at_idx ON dispatched_turns (claimed_at);
