ALTER TABLE session_profiles ADD COLUMN auto_followup JSONB;

CREATE TABLE session_auto_followup_intents (
    spawn_key     TEXT PRIMARY KEY,
    auto_followup JSONB NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
