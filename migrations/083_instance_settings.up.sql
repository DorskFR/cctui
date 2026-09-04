-- Server-wide (instance) settings, admin-editable at runtime. One row per
-- key; the value is free-form JSON so later instance-level knobs need no new
-- table. First key: `name` — the deployment label the webui shows as
-- "cctui (NAME)" in the header and tab title.
CREATE TABLE instance_settings (
    key        TEXT        PRIMARY KEY,
    value      JSONB       NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
