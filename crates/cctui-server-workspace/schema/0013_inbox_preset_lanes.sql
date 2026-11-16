ALTER TABLE presets ADD COLUMN notify_url TEXT;

ALTER TABLE inbox_lanes
    ADD COLUMN spawn_command_id UUID,
    ADD COLUMN paused_reason    TEXT,
    ADD COLUMN ceiling_from     TIMESTAMPTZ,
    ADD COLUMN stuck_alerted_at TIMESTAMPTZ;

CREATE INDEX inbox_events_lane_created ON inbox_events (lane_id, created_at);
