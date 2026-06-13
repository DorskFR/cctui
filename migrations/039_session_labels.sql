-- Session labels (CCT-360): reusable, user-defined colored labels attached to
-- sessions many-to-many. `labels` holds the global label definitions (unique by
-- case-insensitive name); `session_labels` is the junction. Deleting a session
-- or a label cascades to drop the attachments.
CREATE TABLE IF NOT EXISTS labels (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT NOT NULL,
    color      TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One label per name, case-insensitive, so the picker's get-or-create stays
-- idempotent ("Bug" and "bug" are the same label).
CREATE UNIQUE INDEX IF NOT EXISTS labels_name_lower_key ON labels (lower(name));

CREATE TABLE IF NOT EXISTS session_labels (
    session_id TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    label_id   UUID NOT NULL REFERENCES labels (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (session_id, label_id)
);

CREATE INDEX IF NOT EXISTS session_labels_label_idx ON session_labels (label_id);
