-- Enable the codex adapter on machines that enrolled before codex was
-- auto-enabled (CCT-89). New enrollments get both adapters from the enroll
-- route; this backfills existing machines so a fresh Reconcile starts their
-- codex adapter and either harness can be spawned/observed. Safe on machines
-- without codex installed: the adapter only spawns `codex app-server` on an
-- explicit Spawn and its log-tail no-ops when ~/.codex/sessions is absent.
INSERT INTO adapters_enabled (machine_id, adapter_id, config, enabled)
SELECT id, 'codex', '{}'::jsonb, TRUE FROM machines
ON CONFLICT (machine_id, adapter_id) DO NOTHING;
