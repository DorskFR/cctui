-- Bench dataset for the CCT-1006 search-performance measurements.
-- 3000 sessions / 600k stream_events / ~576 MB. See README.md.
\set ON_ERROR_STOP on
\timing on

INSERT INTO users (id, name, key_hash)
VALUES ('11111111-1111-1111-1111-111111111111', 'bench', 'bench-key-hash');

INSERT INTO machines (id, name, key_hash, display_name, hue, kind, user_id)
SELECT gen_random_uuid(), 'bench-m' || g, 'kh-' || g, 'Bench Machine ' || g, (g * 37) % 360, 'worker',
       '11111111-1111-1111-1111-111111111111'
FROM generate_series(1, 12) g;

INSERT INTO sessions (id, machine_id, machine_uuid, working_dir, status, registered_at, last_heartbeat, metadata, adapter_id, session_name, model, effort, pinned)
SELECT
  'bench-sess-' || lpad(g::text, 6, '0'),
  'bench-m' || (1 + g % 12),
  (SELECT id FROM machines WHERE name = 'bench-m' || (1 + g % 12)),
  '/home/dorsk/Documents/repo' || (g % 40),
  CASE WHEN g % 10 = 0 THEN 'active' ELSE 'archived' END,
  now() - (g || ' minutes')::interval,
  now() - (g || ' minutes')::interval,
  jsonb_build_object('model', 'claude-opus-5'),
  'claude-code',
  'Session ' || g || ' ' || md5(g::text),
  CASE WHEN g % 3 = 0 THEN 'claude-opus-5' ELSE 'claude-sonnet-5' END,
  CASE WHEN g % 4 = 0 THEN 'high' ELSE 'medium' END,
  g % 50 = 0
FROM generate_series(1, 3000) g;

-- ~200 events per session, payload text sized like real transcripts (a few KB,
-- with a long tail past the 8 KiB search cap).
INSERT INTO stream_events (session_id, event_type, payload, created_at)
SELECT
  'bench-sess-' || lpad(s::text, 6, '0'),
  CASE WHEN e % 3 = 0 THEN 'tool_use' ELSE 'message' END,
  jsonb_build_object(
    'text',
    -- Repeat an md5 soup to reach realistic length; sprinkle marker words with
    -- deliberately different selectivity so the bench can probe common vs rare.
    repeat(md5((s * 1000 + e)::text) || ' ' || md5((s * 7 + e * 13)::text) || ' function handler request ', 40)
      || CASE WHEN e % 2 = 0 THEN ' error ' ELSE ' ok ' END
      || CASE WHEN (s * 200 + e) % 5000 = 0 THEN ' zygomorphic ' ELSE '' END
      || CASE WHEN (s * 200 + e) % 97 = 0 THEN ' quokka ' ELSE '' END,
    'tool', CASE WHEN e % 3 = 0 THEN 'Bash' ELSE '' END
  ),
  now() - ((3000 - s) || ' minutes')::interval + (e || ' seconds')::interval
FROM generate_series(1, 3000) s, generate_series(1, 200) e;

INSERT INTO labels (id, name, color) SELECT gen_random_uuid(), 'tag' || g, '#abcdef' FROM generate_series(1, 20) g;
INSERT INTO session_labels (session_id, label_id)
SELECT s.id, l.id FROM sessions s JOIN labels l ON l.name = 'tag' || (1 + (abs(hashtext(s.id)) % 20))
WHERE s.id LIKE 'bench-sess-%';

ANALYZE;
