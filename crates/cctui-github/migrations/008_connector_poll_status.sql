-- CCT-396: surface reconcile-poll health on the connector itself.
--
-- The poll loop (GH-CONN-4) previously logged failures only to the server log,
-- so a bad PAT, missing repo access, or rate-limit left the webui with no feedback
-- at all. Record the last poll attempt's time and (if it failed) the error text so
-- the connector list can show "polled <relative>" and a danger-toned error.
ALTER TABLE connectors ADD COLUMN last_polled_at TIMESTAMPTZ;
ALTER TABLE connectors ADD COLUMN last_error TEXT;
