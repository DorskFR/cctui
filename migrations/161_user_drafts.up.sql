-- Unsent text the user would miss, server-side so it roams between the webui
-- and the TUI: composer drafts per session, the spawn form's autosaved payload,
-- and the recall histories. Keys are opaque client strings (`cctui_draft_<id>`,
-- `cctui_spawn_draft<US><machine><US><cwd>`, …); the server never parses them.
CREATE TABLE user_drafts (
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    text       TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, key)
);

CREATE INDEX user_drafts_user_updated_idx ON user_drafts (user_id, updated_at DESC);
