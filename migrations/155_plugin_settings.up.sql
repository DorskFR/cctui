-- Instance-level (admin) settings for a plugin, plus the per-plugin secret the
-- backend proxy signs identity headers with. Keyed by plugin id rather than
-- carried on `plugins`, so a read-only `CCTUI_PLUGINS_DIR` plugin — which has
-- no `plugins` row — can still be configured.
CREATE TABLE plugin_settings (
    plugin_id      TEXT PRIMARY KEY,
    setting_values JSONB NOT NULL DEFAULT '{}'::jsonb,
    proxy_secret   TEXT,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
