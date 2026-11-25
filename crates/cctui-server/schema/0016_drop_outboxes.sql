-- Drops the outboxes 0014 folded into the inbox, refusing while a row the
-- fold carries has neither its inbox event nor its delivered turn.

SELECT pg_advisory_xact_lock(hashtext('cctui.fold_outboxes'));

DO $$
DECLARE
    messages bigint;
    drafts bigint;
BEGIN
    SELECT count(*) INTO messages
    FROM session_message_queue q
    WHERE q.state IN ('scheduled', 'sending', 'dead')
      AND NOT EXISTS (SELECT 1 FROM inbox_events e WHERE e.id = q.id)
      AND NOT EXISTS (SELECT 1 FROM scheduled_turns t WHERE t.event_id = q.id);

    SELECT count(*) INTO drafts
    FROM draft_launch_queue d
    WHERE d.state IN ('scheduled', 'launching', 'dead')
      AND NOT EXISTS (
          SELECT 1
          FROM inbox_events e
          JOIN inbox_lanes l ON l.id = e.lane_id
          WHERE e.source = 'timer' AND e.source_id = d.draft_id
            AND l.key = 'draft:' || d.draft_id);

    IF messages > 0 OR drafts > 0 THEN
        RAISE EXCEPTION 'refusing to drop the legacy outboxes: % scheduled messages and % draft launches are not in the inbox',
            messages, drafts;
    END IF;
END
$$;

DROP TABLE session_message_queue;
DROP TABLE draft_launch_queue;
