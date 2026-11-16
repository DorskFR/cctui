-- Folds session_message_queue and draft_launch_queue into the inbox. Both
-- tables stay, unwritten, until the next release drops them. Re-run on a timer
-- for rows an old replica writes mid-deploy, so every statement must stay
-- idempotent; a delivered message, recorded in scheduled_turns, is never folded again once
-- the inbox prunes its event.

CREATE TABLE IF NOT EXISTS scheduled_turns (
    turn_id      UUID PRIMARY KEY,
    session_id   TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    event_id     UUID,
    scheduled_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS scheduled_turns_session ON scheduled_turns (session_id);
CREATE INDEX IF NOT EXISTS scheduled_turns_event ON scheduled_turns (event_id);

INSERT INTO inbox_lanes (user_id, key, session_id)
SELECT DISTINCT COALESCE(m.user_id, s.user_id, q.user_id), 'session:' || q.session_id, q.session_id
FROM session_message_queue q
JOIN sessions s ON s.id = q.session_id
LEFT JOIN machines m ON m.id = s.machine_uuid
WHERE (q.state IN ('scheduled', 'sending', 'dead')
       OR (q.state = 'sent' AND q.sent_at > now() - interval '1 day'))
  AND COALESCE(m.user_id, s.user_id, q.user_id) IS NOT NULL
  AND NOT EXISTS (SELECT 1 FROM scheduled_turns t WHERE t.event_id = q.id)
ORDER BY 1, 2
ON CONFLICT (user_id, key) DO NOTHING;

INSERT INTO inbox_events
    (id, lane_id, user_id, source, body, sealed_body, payload, status, not_before, attempts,
     next_attempt_at, last_error, batch_id, delivered_session_id, delivered_as, created_at,
     delivered_at)
SELECT q.id, l.id, COALESCE(q.user_id, l.user_id), 'human', q.body, q.sealed_body,
       jsonb_strip_nulls(jsonb_build_object(
           'kind', 'scheduled_message', 'origin', q.origin, 'ask_picks', q.ask_picks)),
       CASE q.state
           WHEN 'scheduled' THEN 'pending'
           WHEN 'sending' THEN 'claimed'
           WHEN 'sent' THEN 'delivered'
           ELSE q.state
       END,
       q.deliver_at, q.attempts, q.next_attempt_at, q.last_error, q.turn_id,
       CASE WHEN q.state = 'sent' THEN q.session_id END,
       CASE WHEN q.state = 'sent' THEN 'reply' END,
       q.created_at,
       CASE WHEN q.state = 'sent' THEN q.sent_at END
FROM session_message_queue q
JOIN sessions s ON s.id = q.session_id
LEFT JOIN machines m ON m.id = s.machine_uuid
JOIN inbox_lanes l
  ON l.user_id = COALESCE(m.user_id, s.user_id, q.user_id) AND l.key = 'session:' || q.session_id
WHERE (q.state IN ('scheduled', 'sending', 'dead')
       OR (q.state = 'sent' AND q.sent_at > now() - interval '1 day'))
  AND NOT EXISTS (SELECT 1 FROM scheduled_turns t WHERE t.event_id = q.id)
ORDER BY q.deliver_at
ON CONFLICT DO NOTHING;

INSERT INTO scheduled_turns (turn_id, session_id, event_id, scheduled_at)
SELECT q.turn_id, q.session_id, q.id, q.deliver_at
FROM session_message_queue q
WHERE q.state = 'sent'
ON CONFLICT (turn_id) DO NOTHING;

INSERT INTO inbox_lanes (user_id, key)
SELECT DISTINCT COALESCE(m.user_id, s.user_id, d.user_id), 'draft:' || d.draft_id
FROM draft_launch_queue d
JOIN sessions s ON s.id = d.draft_id
LEFT JOIN machines m ON m.id = s.machine_uuid
WHERE d.state IN ('scheduled', 'launching', 'dead')
  AND COALESCE(m.user_id, s.user_id, d.user_id) IS NOT NULL
ORDER BY 1, 2
ON CONFLICT (user_id, key) DO NOTHING;

INSERT INTO inbox_events
    (lane_id, user_id, source, source_id, body, payload, status, not_before, attempts,
     next_attempt_at, last_error, batch_id, created_at)
SELECT l.id, COALESCE(d.user_id, l.user_id), 'timer', d.draft_id, 'launch the scheduled draft',
       '{"kind": "draft_launch"}'::jsonb,
       CASE d.state WHEN 'scheduled' THEN 'pending' WHEN 'launching' THEN 'claimed' ELSE d.state END,
       d.launch_at, d.attempts, d.next_attempt_at, d.last_error,
       CASE WHEN d.state = 'launching' THEN gen_random_uuid() END,
       d.created_at
FROM draft_launch_queue d
JOIN sessions s ON s.id = d.draft_id
LEFT JOIN machines m ON m.id = s.machine_uuid
JOIN inbox_lanes l
  ON l.user_id = COALESCE(m.user_id, s.user_id, d.user_id) AND l.key = 'draft:' || d.draft_id
WHERE d.state IN ('scheduled', 'launching', 'dead')
ORDER BY d.launch_at
ON CONFLICT DO NOTHING;
