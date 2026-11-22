CREATE TABLE presets (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    prompt          TEXT NOT NULL,
    machine_id      UUID,
    working_dir     TEXT,
    adapter_id      TEXT,
    model           TEXT,
    permission_mode TEXT CHECK (permission_mode IN ('ask', 'auto', 'yolo', 'whip')),
    dispatcher_id   UUID,
    profile         TEXT,
    external_sources BOOLEAN NOT NULL DEFAULT false,
    fresh_secs      INTEGER NOT NULL DEFAULT 900 CHECK (fresh_secs BETWEEN 60 AND 2700),
    on_stale        TEXT NOT NULL DEFAULT 'followup' CHECK (on_stale IN ('followup', 'new')),
    max_batch       INTEGER NOT NULL DEFAULT 20 CHECK (max_batch BETWEEN 1 AND 100),
    cron            TEXT,
    next_fire_at    TIMESTAMPTZ,
    then_preset_id  UUID REFERENCES presets(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, name),
    CHECK (then_preset_id IS DISTINCT FROM id),
    CHECK (
        (machine_id IS NOT NULL AND working_dir IS NOT NULL AND adapter_id IS NOT NULL
         AND dispatcher_id IS NULL AND profile IS NULL)
        OR
        (dispatcher_id IS NOT NULL AND profile IS NOT NULL
         AND machine_id IS NULL AND working_dir IS NULL AND adapter_id IS NULL
         AND model IS NULL AND permission_mode IS NULL)
    ),
    CHECK (NOT external_sources OR machine_id IS NULL
           OR COALESCE(permission_mode IN ('ask', 'auto'), false))
);

CREATE INDEX presets_next_fire ON presets (next_fire_at) WHERE next_fire_at IS NOT NULL;

DELETE FROM inbox_lanes WHERE preset_id IS NOT NULL;

ALTER TABLE inbox_lanes
    ADD CONSTRAINT inbox_lanes_preset_id_fkey
    FOREIGN KEY (preset_id) REFERENCES presets(id) ON DELETE CASCADE;

CREATE INDEX inbox_lanes_preset ON inbox_lanes (preset_id) WHERE preset_id IS NOT NULL;
