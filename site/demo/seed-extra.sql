-- Extra invented fixture for the marketing clips, applied after the base
-- deploy/local/fixture/seed.sql. Re-running it is a no-op: it clears its own
-- rows first. Rows carrying a live prompt (permission, question, plan) cannot
-- be seeded; site/demo/live-driver.mjs produces those.
--
--   psql -v voice="$(base64 -w0 note.ogg)" -f site/demo/seed-extra.sql

BEGIN;

\set usr '00000000-0000-0000-0000-000000000000'
\set ws1 'c0000000-0000-4000-8000-000000000001'
\set ci2 'c0000000-0000-4000-8000-000000000002'
\set bb3 'c0000000-0000-4000-8000-000000000003'
\set s1 'a0000000-0000-4000-8000-000000000001'
\set s2 'a0000000-0000-4000-8000-000000000002'
\set s3 'a0000000-0000-4000-8000-000000000003'
\set s4 'a0000000-0000-4000-8000-000000000004'
\set s5 'a0000000-0000-4000-8000-000000000005'
\set s7 'a0000000-0000-4000-8000-000000000007'
\set s8 'a0000000-0000-4000-8000-000000000008'
\set s9 'a0000000-0000-4000-8000-000000000009'
\set room 'e0000000-0000-4000-8000-000000000001'

DELETE FROM stream_events WHERE session_id IN (:'s4', :'s7', :'s8', :'s9');
DELETE FROM stream_events WHERE session_id = :'s3' AND payload ? 'demo_page';
DELETE FROM stream_events WHERE session_id IN (:'s1', :'s5') AND payload->>'text' LIKE '<cctui-%';
DELETE FROM stream_events WHERE session_id = :'s5' AND payload->>'role' = 'system_marker';
DELETE FROM stream_events WHERE session_id = :'s1' AND payload->>'role' IN ('assistant_voice');
DELETE FROM stream_events WHERE session_id = :'s1' AND payload->>'text' LIKE '<cctui-session-context voice%';
DELETE FROM session_voice_notes WHERE session_id LIKE 'a0000000-%';
DELETE FROM session_user_actions WHERE session_id LIKE 'a0000000-%';
DELETE FROM guard_steps WHERE session_id LIKE 'a0000000-%';
DELETE FROM bookmarks WHERE id::text LIKE 'e1000000-%';
DELETE FROM room_messages WHERE room_id = :'room';
UPDATE sessions SET room_id = NULL WHERE room_id = :'room';
DELETE FROM rooms WHERE id = :'room';
DELETE FROM dispatch_records WHERE dispatcher_name = 'kube-eu';
DELETE FROM dispatchers WHERE id::text LIKE 'e2000000-%';
DELETE FROM context_items WHERE id::text LIKE 'e3000000-%';
DELETE FROM events WHERE detail ? 'demo';
DELETE FROM session_labels WHERE session_id IN (:'s7', :'s8', :'s9');
DELETE FROM session_token_usage WHERE session_id IN (:'s7', :'s8', :'s9');
DELETE FROM sessions WHERE id IN (:'s7', :'s8', :'s9');
DELETE FROM machines WHERE id = :'bb3';

-- A derivable key for ci-runner-02 so a second fake daemon can drive it, and a
-- third machine that has been offline for two days.
UPDATE machines SET key_hash = encode(sha256('cctui_m_seed-fixture-ci-runner-02'::bytea), 'hex'),
  key_preview = 'cctui_m_…nner-02'
  WHERE id = :'ci2';
UPDATE machines SET key_preview = 'cctui_m_…tion-01' WHERE id = :'ws1';
INSERT INTO machines (id, user_id, name, key_hash, kind, hue, display_name, first_seen_at, last_seen_at, key_preview)
VALUES (:'bb3', :'usr', 'build-box-03', 'seed-fixture-machine-3', 'persistent', 20, 'build-box-03',
        now() - interval '60 days', now() - interval '2 days', 'cctui_m_…box-03');

-- Two more harnesses: an opencode session and a Gemini session over ACP.
INSERT INTO sessions (
  id, parent_id, machine_id, machine_uuid, working_dir, status, registered_at,
  last_heartbeat, metadata, user_id, adapter_id, tempo, agent_state, activity,
  session_name, model, effort, pinned, last_tool_at, last_tool_name,
  tool_use_count, children, ended_at, end_reason
) VALUES
  (:'s7', NULL, 'workstation-01', :'ws1', '/work/acme/storefront', 'active',
   now() - interval '52 minutes', now() - interval '2 minutes',
   '{"title":"Migrate the cart store to signals"}'::jsonb, :'usr', 'opencode',
   'idle', 'idle', 'Ready for review', 'Migrate the cart store to signals',
   'kimi-k2.5', NULL, false, now() - interval '2 minutes', 'edit', 11, '[]'::jsonb, NULL, NULL),
  (:'s8', NULL, 'ci-runner-02', :'ci2', '/work/acme/docs-site', 'active',
   now() - interval '3 hours', now() - interval '41 minutes',
   '{"title":"Draft the 2.4 release notes"}'::jsonb, :'usr', 'gemini',
   'idle', 'done', 'success', 'Draft the 2.4 release notes',
   'gemini-3.1-pro', NULL, false, now() - interval '41 minutes', 'write_file', 5, '[]'::jsonb,
   now() - interval '41 minutes', 'completed'),
  -- The session a failed worker dispatch would have become.
  (:'s9', NULL, 'kube-eu', NULL, '/workspace/acme/billing', 'failed',
   now() - interval '23 minutes', now() - interval '23 minutes',
   '{"title":"Backfill invoice totals","origin":"dispatch"}'::jsonb, :'usr', 'claude-code',
   NULL, 'failed', 'failure', 'Backfill invoice totals', 'sonnet', 'medium', false,
   NULL, NULL, 0, '[]'::jsonb, now() - interval '22 minutes', 'crashed');

INSERT INTO session_labels (session_id, label_id) VALUES
  (:'s7', 'b0000000-0000-4000-8000-000000000002'),
  (:'s8', 'b0000000-0000-4000-8000-000000000002'),
  (:'s9', 'b0000000-0000-4000-8000-000000000001');

INSERT INTO stream_events (session_id, event_type, payload, created_at) VALUES
  (:'s7', 'message', '{"type":"text","role":"User","content":"Move the cart store from the event emitter to signals. Keep the public API."}'::jsonb, now() - interval '52 minutes'),
  (:'s7', 'message', '{"type":"text","role":"Assistant","kind":"thinking","content":"The emitter has three subscribers: the header badge, the drawer and the checkout button. Signals can replace all three with one computed total."}'::jsonb, now() - interval '51 minutes'),
  (:'s7', 'message', '{"type":"text","role":"Assistant","content":"Reading the current store and its three subscribers first."}'::jsonb, now() - interval '51 minutes'),
  (:'s7', 'tool_use', '{"type":"tool_call","tool":"read","input":{"filePath":"/work/acme/storefront/src/lib/cart.ts"},"tool_use_id":"oc_1"}'::jsonb, now() - interval '50 minutes'),
  (:'s7', 'tool_use', '{"type":"tool_result","tool_use_id":"oc_1","output_summary":"export const cart = createEmitter<CartState>() … (62 lines)"}'::jsonb, now() - interval '50 minutes'),
  (:'s7', 'tool_use', '{"type":"tool_call","tool":"edit","input":{"filePath":"/work/acme/storefront/src/lib/cart.ts","oldString":"export const cart = createEmitter<CartState>()","newString":"export const cart = signal<CartState>({ items: [] })\nexport const total = computed(() => cart().items.reduce((n, i) => n + i.price * i.qty, 0))"},"tool_use_id":"oc_2"}'::jsonb, now() - interval '31 minutes'),
  (:'s7', 'tool_use', '{"type":"tool_result","tool_use_id":"oc_2","output_summary":"Edited src/lib/cart.ts"}'::jsonb, now() - interval '31 minutes'),
  (:'s7', 'tool_use', '{"type":"tool_call","tool":"bash","input":{"command":"npm test -- cart","description":"Run the cart tests"},"tool_use_id":"oc_3"}'::jsonb, now() - interval '4 minutes'),
  (:'s7', 'tool_use', '{"type":"tool_result","tool_use_id":"oc_3","output_summary":"Test Files  2 passed (2)\n     Tests  14 passed (14)"}'::jsonb, now() - interval '3 minutes'),
  (:'s7', 'message', '{"type":"text","role":"Assistant","content":"The cart store is now a signal with a computed `total`; the badge, drawer and checkout button read it directly. All 14 cart tests pass. Ready for review."}'::jsonb, now() - interval '2 minutes'),
  (:'s8', 'message', '{"type":"text","role":"User","content":"Draft the release notes for 2.4 from the merged pull requests since the 2.3 tag."}'::jsonb, now() - interval '3 hours'),
  (:'s8', 'tool_use', '{"type":"tool_call","tool":"run_shell_command","input":{"command":"git log v2.3..HEAD --merges --format=%s"},"tool_use_id":"g_1"}'::jsonb, now() - interval '179 minutes'),
  (:'s8', 'tool_use', '{"type":"tool_result","tool_use_id":"g_1","output_summary":"Merge pull request #412: cursor pagination for orders\nMerge pull request #418: signals cart store\nMerge pull request #421: rotate staging credentials"}'::jsonb, now() - interval '179 minutes'),
  (:'s8', 'message', '{"type":"text","role":"Assistant","kind":"thinking","content":"Three merged changes, one of them operational. Group them as features and maintenance."}'::jsonb, now() - interval '178 minutes'),
  (:'s8', 'tool_use', '{"type":"tool_call","tool":"write_file","input":{"path":"/work/acme/docs-site/releases/2.4.md","content":"# 2.4\n\n## Features\n- Cursor pagination on GET /orders\n- Cart store rebuilt on signals\n\n## Maintenance\n- Staging credentials rotated\n"},"tool_use_id":"g_2"}'::jsonb, now() - interval '42 minutes'),
  (:'s8', 'tool_use', '{"type":"tool_result","tool_use_id":"g_2","output_summary":"Wrote releases/2.4.md"}'::jsonb, now() - interval '42 minutes'),
  (:'s8', 'message', '{"type":"text","role":"Assistant","content":"Release notes drafted at releases/2.4.md: two features and one maintenance item."}'::jsonb, now() - interval '41 minutes');

-- The codex session in its own rollout dialect.
INSERT INTO stream_events (session_id, event_type, payload, created_at) VALUES
  (:'s4', 'message', '{"type":"event_msg","payload":{"type":"user_message","message":"Update the onboarding copy so the first screen explains what a workspace is."}}'::jsonb, now() - interval '1 day'),
  (:'s4', 'message', '{"type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"The first screen currently assumes the reader knows the product. One sentence on workspaces before the call to action is enough."}]}}'::jsonb, now() - interval '1 day' + interval '1 minute'),
  (:'s4', 'tool_use', '{"type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"cx_1","arguments":"{\"command\":[\"rg\",\"-n\",\"Welcome\",\"src/onboarding\"]}"}}'::jsonb, now() - interval '1 day' + interval '2 minutes'),
  (:'s4', 'tool_use', '{"type":"response_item","payload":{"type":"function_call_output","call_id":"cx_1","output":"src/onboarding/Welcome.tsx:14:  <h1>Welcome to acme</h1>\nsrc/onboarding/Welcome.tsx:15:  <p>Get started by creating a project.</p>"}}'::jsonb, now() - interval '1 day' + interval '2 minutes'),
  (:'s4', 'tool_use', '{"type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","call_id":"cx_2","input":"*** Begin Patch\n*** Update File: src/onboarding/Welcome.tsx\n@@\n-  <p>Get started by creating a project.</p>\n+  <p>A workspace holds your projects, teammates and billing in one place. Create your first one to get started.</p>\n*** End Patch"}}'::jsonb, now() - interval '20 hours' - interval '3 minutes'),
  (:'s4', 'tool_use', '{"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"cx_2","output":"Success. Updated the following files:\nM src/onboarding/Welcome.tsx"}}'::jsonb, now() - interval '20 hours' - interval '3 minutes'),
  (:'s4', 'message', '{"type":"event_msg","payload":{"type":"agent_message","message":"The first onboarding screen now explains what a workspace is before asking the reader to create one. No other copy changed."}}'::jsonb, now() - interval '20 hours');

INSERT INTO session_token_usage
  (session_id, message_id, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, created_at, model)
VALUES
  (:'s7', 'msg_seed_07', 2800, 910, 44000, 7000, now() - interval '2 minutes', 'kimi-k2.5'),
  (:'s8', 'msg_seed_08', 3300, 560, 12000, 4000, now() - interval '41 minutes', 'gemini-3.1-pro');

-- A long history on session 3 so older pages load on scroll.
INSERT INTO stream_events (session_id, event_type, payload, created_at)
SELECT :'s3', 'tool_use',
  jsonb_build_object('tool', 'Bash', 'demo_page', true,
    'input', jsonb_build_object('command', format('kubectl -n staging get secret db-credentials-%s -o yaml', i), 'description', format('Inspect secret revision %s', i)),
    'id', format('toolu_page_%s', i)),
  now() - interval '4 hours' + (i * interval '40 seconds')
FROM generate_series(1, 100) AS i;
INSERT INTO stream_events (session_id, event_type, payload, created_at)
SELECT :'s3', 'tool_use',
  jsonb_build_object('kind', 'tool_result', 'demo_page', true, 'tool_use_id', format('toolu_page_%s', i),
    'content', format('apiVersion: v1%skind: Secret%smetadata:%s  name: db-credentials-%s%s  revision: %s', E'\n', E'\n', E'\n', i, E'\n', i)),
  now() - interval '4 hours' + (i * interval '40 seconds') + interval '5 seconds'
FROM generate_series(1, 100) AS i;

-- Room shared by the orchestrator and its subagent, with a short exchange.
INSERT INTO rooms (id, user_id, name, next_seq, created_at)
VALUES (:'room', :'usr', 'checkout-pagination', 3, now() - interval '11 minutes');
UPDATE sessions SET room_id = :'room' WHERE id IN (:'s1', :'s5');
INSERT INTO room_messages (room_id, seq, sender_session_id, sender_label, body, created_at) VALUES
  (:'room', 1, :'s1', 'Add pagination to the orders endpoint (claude-code on workstation-01)',
   'Cursor is keyed on (created_at, id). Please cover the empty page and a stale cursor.', now() - interval '10 minutes'),
  (:'room', 2, :'s5', 'Audit the orders endpoint tests (claude-code on workstation-01)',
   'On it. The stale cursor should 400 rather than return page one, agreed?', now() - interval '8 minutes'),
  (:'room', 3, NULL, 'you (human)', 'Agreed, 400 with a clear error body.', now() - interval '7 minutes');
INSERT INTO stream_events (session_id, event_type, payload, created_at) VALUES
  (:'s5', 'message', jsonb_build_object('role', 'user', 'line_id', 'f0000000-0000-4000-8000-000000000101', 'text',
    E'<cctui-room name="checkout-pagination" from="Add pagination to the orders endpoint (claude-code on workstation-01)">\nCursor is keyed on (created_at, id). Please cover the empty page and a stale cursor.\n</cctui-room>'),
   now() - interval '10 minutes'),
  (:'s1', 'message', jsonb_build_object('role', 'user', 'line_id', 'f0000000-0000-4000-8000-000000000102', 'text',
    E'<cctui-room name="checkout-pagination" from="Audit the orders endpoint tests (claude-code on workstation-01)">\nOn it. The stale cursor should 400 rather than return page one, agreed?\n</cctui-room>'),
   now() - interval '8 minutes'),
  (:'s1', 'message', jsonb_build_object('role', 'user', 'line_id', 'f0000000-0000-4000-8000-000000000103', 'text',
    E'<cctui-room name="checkout-pagination" from="you (human)">\nAgreed, 400 with a clear error body.\n</cctui-room>'),
   now() - interval '7 minutes'),
  (:'s5', 'message', jsonb_build_object('role', 'user', 'line_id', 'f0000000-0000-4000-8000-000000000104', 'text',
    E'<cctui-room name="checkout-pagination" from="you (human)">\nAgreed, 400 with a clear error body.\n</cctui-room>'),
   now() - interval '7 minutes'),
  -- A direct peer message from the subagent to its parent.
  (:'s5', 'message', jsonb_build_object('role', 'system_marker', 'audit_id', 'f0000000-0000-4000-8000-000000000105',
    'text', 'sent a peer message to Add pagination to the orders endpoint [parent]'),
   now() - interval '4 minutes'),
  (:'s1', 'message', jsonb_build_object('role', 'user', 'line_id', 'f0000000-0000-4000-8000-000000000106', 'text',
    E'<cctui-peer-message from="a0000000-0000-4000-8000-000000000005" from-name="Audit the orders endpoint tests">\nThe cursor round-trip test passes. Empty page still returns next_cursor: "" instead of null; I am fixing that in tests/orders.rs.\n</cctui-peer-message>'),
   now() - interval '4 minutes'),
  -- A spoken user turn and the agent's voice note.
  (:'s1', 'message', jsonb_build_object('role', 'user', 'line_id', 'f0000000-0000-4000-8000-000000000107', 'text',
    E'<cctui-session-context voice="spoken">\nThe user spoke this message; reply briefly.\n</cctui-session-context>\n\nKeep the response shape exactly as it is, the mobile app depends on it.'),
   now() - interval '14 minutes');

INSERT INTO session_voice_notes (id, session_id, media_type, byte_len, duration_ms, text, bytes, created_at)
VALUES ('e4000000-0000-4000-8000-000000000001', :'s1', 'audio/ogg', length(decode(:'voice', 'base64')), 2000,
  'Understood. The orders array stays as it is; the cursor rides alongside it.',
  decode(:'voice', 'base64'), now() - interval '13 minutes');
INSERT INTO stream_events (session_id, event_type, payload, created_at) VALUES
  (:'s1', 'message', '{"role":"assistant_voice","text":"Understood. The orders array stays as it is; the cursor rides alongside it.","message_id":"e4000000-0000-4000-8000-000000000001","duration_ms":2000}'::jsonb,
   now() - interval '13 minutes');

INSERT INTO session_user_actions (id, session_id, title, detail, kind, blocking, status, created_at) VALUES
  ('e5000000-0000-4000-8000-000000000001', :'s1', 'Confirm the page size default',
   'The handler defaults to 50 per page. Say if the mobile app needs a different default.', 'decision', false, 'open', now() - interval '16 minutes'),
  ('e5000000-0000-4000-8000-000000000002', :'s1', 'Approve the API reference wording', NULL, 'input', false, 'done', now() - interval '6 minutes');

INSERT INTO guard_steps (session_id, state, reported_at) VALUES
  (:'s3', '{"step":2,"title":"Rotate","allowed":"Bash(kubectl *), Edit(deploy/**)","disallowed":"Bash(git push *)","transitions":[3],"exit":false}'::jsonb,
   now() - interval '26 minutes');

INSERT INTO bookmarks (id, user_id, session_id, seq, message_id, title, body, role, session_name, note, message_ts, created_at) VALUES
  ('e1000000-0000-4000-8000-000000000001', :'usr', :'s1', NULL, 'msg_seed_02', 'Cursor keeps the old response shape',
   'The handler now takes a `?cursor=` parameter and returns the next cursor alongside the existing `orders` array, so current clients keep working.',
   'assistant', 'Add pagination to the orders endpoint', 'Mention in the changelog', now() - interval '19 minutes', now() - interval '18 minutes'),
  ('e1000000-0000-4000-8000-000000000002', :'usr', :'s3', NULL, 'msg_seed_04', 'Staging credentials rotated',
   'Credentials rotated and the deployment updated. Pull request #218 is open and waiting for a review.',
   'assistant', 'Rotate the staging database credentials', NULL, now() - interval '26 minutes', now() - interval '25 minutes'),
  ('e1000000-0000-4000-8000-000000000003', :'usr', :'s2', NULL, 'msg_seed_03', 'Why the checkout test races',
   'Two tests share one seeded cart row, so they race. I can either give each test its own cart or serialise the module.',
   'assistant', 'Fix the flaky checkout test', 'Same pattern in the billing tests?', now() - interval '3 minutes', now() - interval '2 minutes');

INSERT INTO context_items (id, user_id, kind, name, title, body, scope, scope_ref, tags, enabled, version, filters) VALUES
  ('e3000000-0000-4000-8000-000000000001', :'usr', 'rule', 'api-compat', 'Keep public API shapes stable',
   'Never change the shape of an existing JSON response. Add fields; never rename or remove them.', 'path', '/work/acme/checkout-api', '{backend}', true, 1, '{}'::jsonb),
  ('e3000000-0000-4000-8000-000000000002', :'usr', 'memory', 'checkout-cursor', 'Order cursors are (created_at, id)',
   'The orders cursor encodes (created_at, id) as base64; a stale cursor returns 400.', 'path', '/work/acme/checkout-api', '{backend}', true, 1, '{}'::jsonb),
  ('e3000000-0000-4000-8000-000000000003', :'usr', 'prompt', 'review-pr', 'Review a pull request',
   'Review the open pull request on this branch. Flag anything that changes a public API, then summarise in five bullets.', 'user', NULL, '{}', true, 1, '{}'::jsonb);

INSERT INTO dispatchers (id, user_id, name, kind, created_at, updated_at, key_preview, last_seen_at) VALUES
  ('e2000000-0000-4000-8000-000000000001', :'usr', 'kube-eu', 'kube', now() - interval '9 days', now() - interval '9 days', 'cctui_d_…kube-eu', now() - interval '20 seconds'),
  ('e2000000-0000-4000-8000-000000000002', :'usr', 'kube-us', 'kube', now() - interval '9 days', now() - interval '9 days', 'cctui_d_…kube-us', now() - interval '3 hours');

INSERT INTO dispatch_records (session_id, user_id, dispatcher_name, name, profile, repo, model, adapter, prompt_head, prompt_truncated, env_keys, outcome, end_reason, detail, created_at, started_at, worker_exit) VALUES
  (:'s9', :'usr', 'kube-eu', 'Backfill invoice totals', 'worker-large', 'acme/billing', 'sonnet', 'claude-code',
   'Backfill invoice.total for every invoice created before the 2.3 migration, in batches of 500.', false, '{BILLING_DB_URL}', 'failed', 'crashed',
   'worker exited before the session registered', now() - interval '23 minutes', now() - interval '23 minutes',
   '{"container":"worker","exit_code":137,"reason":"OOMKilled","log_tail":"[worker] cloning acme/billing (depth 1)\n[worker] npm ci: added 1412 packages in 38s\n[worker] starting claude-code (sonnet)\n[worker] loading 2.1 GB invoice snapshot into memory\nKilled"}'::jsonb),
  ('a0000000-0000-4000-8000-000000000010', :'usr', 'kube-eu', 'Nightly dependency audit', 'worker-small', 'acme/platform-infra', 'sonnet', 'claude-code',
   'Run the dependency audit and open a pull request for anything with a known fix.', false, '{}', 'registered', NULL, NULL, now() - interval '6 hours', now() - interval '6 hours', NULL);

INSERT INTO events (occurred_at, kind, severity, session_id, machine_id, user_id, actor, summary, detail) VALUES
  (now() - interval '38 minutes', 'session.launched', 'info', :'s1', :'ws1', :'usr', 'user:00000000-0000-0000-0000-000000000000', 'launched Add pagination to the orders endpoint on workstation-01', '{"demo":true,"adapter":"claude-code","model":"opus","session_name":"Add pagination to the orders endpoint","machine_label":"workstation-01"}'::jsonb),
  (now() - interval '12 minutes', 'session.created', 'info', :'s5', :'ws1', :'usr', 'agent:a0000000-0000-4000-8000-000000000001', 'spawned subagent Audit the orders endpoint tests', '{"demo":true,"origin":"child","session_name":"Audit the orders endpoint tests","machine_label":"workstation-01"}'::jsonb),
  (now() - interval '23 minutes', 'session.ended', 'error', :'s9', NULL, :'usr', 'daemon', 'worker for Backfill invoice totals crashed: OOMKilled (exit 137)', '{"demo":true,"dispatcher_id":"kube-eu","session_name":"Backfill invoice totals","end_reason":"crashed","end_detail":"OOMKilled (exit 137)"}'::jsonb),
  (now() - interval '41 minutes', 'session.ended', 'info', :'s8', :'ci2', :'usr', 'daemon', 'Draft the 2.4 release notes completed', '{"demo":true,"end_reason":"completed","session_name":"Draft the 2.4 release notes","machine_label":"ci-runner-02"}'::jsonb),
  (now() - interval '2 days', 'machine.offline', 'warn', NULL, :'bb3', :'usr', 'reaper', 'build-box-03 offline: no heartbeat for 1h', '{"demo":true,"machine_label":"build-box-03"}'::jsonb),
  (now() - interval '9 days', 'system.dispatcher_online', 'info', NULL, NULL, :'usr', 'system', 'dispatcher kube-eu online', '{"demo":true,"dispatcher_id":"kube-eu"}'::jsonb),
  (now() - interval '3 hours', 'system.dispatcher_offline', 'warn', NULL, NULL, :'usr', 'system', 'dispatcher kube-us offline', '{"demo":true,"dispatcher_id":"kube-us"}'::jsonb);

COMMIT;
