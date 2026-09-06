-- Synthetic fixture for the documentation journeys. Every name, path and
-- prompt here is invented; nothing comes from a real deployment. Applying it
-- twice is a no-op — it clears its own rows first.
--
--   psql "$DATABASE_URL" -f webui/journeys/fixtures/seed.sql

BEGIN;

DELETE FROM sessions WHERE status = 'draft';
DELETE FROM stream_events WHERE session_id LIKE 'a0000000-%';
DELETE FROM session_token_usage WHERE session_id LIKE 'a0000000-%';
DELETE FROM session_labels WHERE session_id LIKE 'a0000000-%';
DELETE FROM sessions WHERE id LIKE 'a0000000-%';
DELETE FROM labels WHERE id IN (
  'b0000000-0000-4000-8000-000000000001',
  'b0000000-0000-4000-8000-000000000002',
  'b0000000-0000-4000-8000-000000000003'
);
DELETE FROM machines WHERE id IN (
  'c0000000-0000-4000-8000-000000000001',
  'c0000000-0000-4000-8000-000000000002'
);

INSERT INTO machines (id, user_id, name, key_hash, kind, hue, display_name, first_seen_at, last_seen_at)
VALUES
  ('c0000000-0000-4000-8000-000000000001', '00000000-0000-0000-0000-000000000000',
   'workstation-01', 'seed-fixture-machine-1', 'persistent', 210, 'workstation-01',
   now() - interval '31 days', now()),
  ('c0000000-0000-4000-8000-000000000002', '00000000-0000-0000-0000-000000000000',
   'ci-runner-02', 'seed-fixture-machine-2', 'persistent', 145, 'ci-runner-02',
   now() - interval '12 days', now() - interval '4 minutes');

INSERT INTO labels (id, name, color) VALUES
  ('b0000000-0000-4000-8000-000000000001', 'backend',  '#3b82f6'),
  ('b0000000-0000-4000-8000-000000000002', 'frontend', '#a855f7'),
  ('b0000000-0000-4000-8000-000000000003', 'infra',    '#f59e0b');

INSERT INTO sessions (
  id, parent_id, machine_id, machine_uuid, working_dir, status, registered_at,
  last_heartbeat, metadata, user_id, adapter_id, tempo, agent_state, activity,
  session_name, model, effort, pinned, pinned_at, last_tool_at, last_tool_name,
  tool_use_count, children, ended_at, end_reason
) VALUES
  -- Working, pinned, the star of the follow-session journey.
  ('a0000000-0000-4000-8000-000000000001', NULL, 'workstation-01',
   'c0000000-0000-4000-8000-000000000001', '/work/acme/checkout-api', 'active',
   now() - interval '38 minutes', now() - interval '9 seconds',
   '{"title":"Add pagination to the orders endpoint"}'::jsonb,
   '00000000-0000-0000-0000-000000000000', 'claude-code', 'active', 'working',
   'Editing src/routes/orders.rs', 'Add pagination to the orders endpoint',
   'opus', 'medium', true, now() - interval '30 minutes',
   now() - interval '11 seconds', 'Edit', 14, '[]'::jsonb, NULL, NULL),

  -- Needs input: the reason the Blocked bucket exists.
  ('a0000000-0000-4000-8000-000000000002', NULL, 'workstation-01',
   'c0000000-0000-4000-8000-000000000001', '/work/acme/checkout-api', 'active',
   now() - interval '2 hours', now() - interval '3 minutes',
   '{"title":"Fix the flaky checkout test"}'::jsonb,
   '00000000-0000-0000-0000-000000000000', 'claude-code', 'blocked', 'waiting',
   'Waiting for an answer', 'Fix the flaky checkout test', 'sonnet', NULL,
   false, NULL, now() - interval '3 minutes', 'Bash', 6, '[]'::jsonb, NULL, NULL),

  -- Ready for review: carries a linked pull request.
  ('a0000000-0000-4000-8000-000000000003', NULL, 'ci-runner-02',
   'c0000000-0000-4000-8000-000000000002', '/work/acme/platform-infra', 'active',
   now() - interval '5 hours', now() - interval '26 minutes',
   '{"title":"Rotate the staging database credentials"}'::jsonb,
   '00000000-0000-0000-0000-000000000000', 'claude-code', 'idle', 'idle',
   NULL, 'Rotate the staging database credentials', 'opus', 'high',
   false, NULL, now() - interval '26 minutes', 'Bash', 21,
   '[{"kind":"pr","href":"https://github.com/acme/platform-infra/pull/218","title":"Rotate staging database credentials"}]'::jsonb,
   NULL, NULL),

  -- Completed, on a second adapter, so the docs show more than one harness.
  ('a0000000-0000-4000-8000-000000000004', NULL, 'workstation-01',
   'c0000000-0000-4000-8000-000000000001', '/work/acme/storefront', 'inactive',
   now() - interval '1 day', now() - interval '20 hours',
   '{"title":"Update the onboarding copy"}'::jsonb,
   '00000000-0000-0000-0000-000000000000', 'codex', 'idle', 'done', 'success',
   'Update the onboarding copy', 'gpt-5-codex', 'medium', false, NULL,
   now() - interval '20 hours', 'apply_patch', 9, '[]'::jsonb,
   now() - interval '20 hours', 'completed'),

  -- A subagent of the first session, so the parent/child rollup is visible.
  ('a0000000-0000-4000-8000-000000000005', 'a0000000-0000-4000-8000-000000000001',
   'workstation-01', 'c0000000-0000-4000-8000-000000000001',
   '/work/acme/checkout-api', 'active',
   now() - interval '12 minutes', now() - interval '40 seconds',
   '{"title":"Audit the orders endpoint tests"}'::jsonb,
   '00000000-0000-0000-0000-000000000000', 'claude-code', 'active', 'working',
   'Running the orders test suite', 'Audit the orders endpoint tests',
   'sonnet', 'low', false, NULL, now() - interval '40 seconds', 'Bash', 4,
   '[]'::jsonb, NULL, NULL);

INSERT INTO session_labels (session_id, label_id) VALUES
  ('a0000000-0000-4000-8000-000000000001', 'b0000000-0000-4000-8000-000000000001'),
  ('a0000000-0000-4000-8000-000000000002', 'b0000000-0000-4000-8000-000000000001'),
  ('a0000000-0000-4000-8000-000000000003', 'b0000000-0000-4000-8000-000000000003'),
  ('a0000000-0000-4000-8000-000000000004', 'b0000000-0000-4000-8000-000000000002');

-- Conversation for the pinned session: a prompt, a plan, four tool calls and a
-- closing summary — enough for the timeline and the expandable tool call.
INSERT INTO stream_events (session_id, event_type, payload, created_at) VALUES
  ('a0000000-0000-4000-8000-000000000001', 'message',
   '{"role":"user","text":"Add cursor pagination to GET /orders. Keep the current response shape and cover it with a test."}'::jsonb,
   now() - interval '38 minutes'),
  ('a0000000-0000-4000-8000-000000000001', 'message',
   '{"role":"assistant_thinking","text":"The handler returns the whole table today. A cursor keyed on (created_at, id) keeps the page stable while orders are inserted."}'::jsonb,
   now() - interval '37 minutes'),
  ('a0000000-0000-4000-8000-000000000001', 'message',
   '{"role":"assistant","text":"I will read the orders handler first, then add a cursor parameter and a test.","message_id":"msg_seed_01"}'::jsonb,
   now() - interval '37 minutes'),
  ('a0000000-0000-4000-8000-000000000001', 'tool_use',
   '{"tool":"Read","input":{"file_path":"/work/acme/checkout-api/src/routes/orders.rs"}}'::jsonb,
   now() - interval '36 minutes'),
  ('a0000000-0000-4000-8000-000000000001', 'tool_use',
   '{"kind":"tool_result","content":"pub async fn list_orders(State(db): State<Db>) -> Json<Vec<Order>> { … }  (84 lines)"}'::jsonb,
   now() - interval '36 minutes'),
  ('a0000000-0000-4000-8000-000000000001', 'tool_use',
   '{"tool":"Grep","input":{"pattern":"list_orders","path":"/work/acme/checkout-api"}}'::jsonb,
   now() - interval '35 minutes'),
  ('a0000000-0000-4000-8000-000000000001', 'tool_use',
   '{"kind":"tool_result","content":"src/routes/orders.rs:31\nsrc/routes/mod.rs:12\ntests/orders.rs:8"}'::jsonb,
   now() - interval '35 minutes'),
  ('a0000000-0000-4000-8000-000000000001', 'tool_use',
   '{"tool":"Edit","input":{"file_path":"/work/acme/checkout-api/src/routes/orders.rs","old_string":"pub async fn list_orders(State(db): State<Db>)","new_string":"pub async fn list_orders(State(db): State<Db>, Query(page): Query<Cursor>)"}}'::jsonb,
   now() - interval '20 minutes'),
  ('a0000000-0000-4000-8000-000000000001', 'tool_use',
   '{"kind":"tool_result","content":"Applied 1 edit to src/routes/orders.rs"}'::jsonb,
   now() - interval '20 minutes'),
  ('a0000000-0000-4000-8000-000000000001', 'message',
   '{"role":"assistant","text":"The handler now takes a `?cursor=` parameter and returns the next cursor alongside the existing `orders` array, so current clients keep working. Next I am adding a test that walks two pages.","message_id":"msg_seed_02"}'::jsonb,
   now() - interval '19 minutes'),
  ('a0000000-0000-4000-8000-000000000001', 'tool_use',
   '{"tool":"Bash","input":{"command":"cargo test -p checkout-api orders","description":"Run the orders tests"}}'::jsonb,
   now() - interval '11 seconds'),
  ('a0000000-0000-4000-8000-000000000001', 'message',
   '{"role":"summary","status_detail":"Pagination added; running the orders tests","status_category":"working","needs_action":false}'::jsonb,
   now() - interval '9 seconds');

-- The blocked session ends on a question, which is what puts it in Needs input.
INSERT INTO stream_events (session_id, event_type, payload, created_at) VALUES
  ('a0000000-0000-4000-8000-000000000002', 'message',
   '{"role":"user","text":"The checkout test fails about one run in five on CI. Find out why."}'::jsonb,
   now() - interval '2 hours'),
  ('a0000000-0000-4000-8000-000000000002', 'tool_use',
   '{"tool":"Bash","input":{"command":"cargo test checkout -- --test-threads=1","description":"Reproduce the failure"}}'::jsonb,
   now() - interval '119 minutes'),
  ('a0000000-0000-4000-8000-000000000002', 'tool_use',
   '{"kind":"tool_result","content":"test checkout::totals_match ... FAILED (1 of 20 runs)"}'::jsonb,
   now() - interval '118 minutes'),
  ('a0000000-0000-4000-8000-000000000002', 'message',
   '{"role":"assistant","text":"Two tests share one seeded cart row, so they race. I can either give each test its own cart or serialise the module. Which do you want?","message_id":"msg_seed_03"}'::jsonb,
   now() - interval '3 minutes'),
  ('a0000000-0000-4000-8000-000000000002', 'message',
   '{"role":"summary","status_detail":"Waiting on a choice between isolating the fixture and serialising the module","status_category":"waiting","needs_action":true}'::jsonb,
   now() - interval '3 minutes');

INSERT INTO stream_events (session_id, event_type, payload, created_at) VALUES
  ('a0000000-0000-4000-8000-000000000003', 'message',
   '{"role":"user","text":"Rotate the staging database credentials and open a pull request."}'::jsonb,
   now() - interval '5 hours'),
  ('a0000000-0000-4000-8000-000000000003', 'message',
   '{"role":"assistant","text":"Credentials rotated and the deployment updated. Pull request #218 is open and waiting for a review.","message_id":"msg_seed_04"}'::jsonb,
   now() - interval '26 minutes');

INSERT INTO stream_events (session_id, event_type, payload, created_at) VALUES
  ('a0000000-0000-4000-8000-000000000005', 'message',
   '{"role":"user","text":"Check that the orders tests cover an empty page and a stale cursor."}'::jsonb,
   now() - interval '12 minutes'),
  ('a0000000-0000-4000-8000-000000000005', 'tool_use',
   '{"tool":"Bash","input":{"command":"cargo test -p checkout-api orders::cursor","description":"Run the cursor tests"}}'::jsonb,
   now() - interval '40 seconds');

-- Token usage: what the overview charts and the per-session cost read.
INSERT INTO session_token_usage
  (session_id, message_id, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, created_at, model)
VALUES
  ('a0000000-0000-4000-8000-000000000001', 'msg_seed_01', 4200, 780, 61000, 12000, now() - interval '37 minutes', 'opus'),
  ('a0000000-0000-4000-8000-000000000001', 'msg_seed_02', 3100, 640, 74000,     0, now() - interval '19 minutes', 'opus'),
  ('a0000000-0000-4000-8000-000000000002', 'msg_seed_03', 2600, 410, 38000,  9000, now() - interval '3 minutes',  'sonnet'),
  ('a0000000-0000-4000-8000-000000000003', 'msg_seed_04', 5400, 1220, 88000, 15000, now() - interval '26 minutes', 'opus'),
  ('a0000000-0000-4000-8000-000000000004', 'msg_seed_05', 1900, 350, 21000,  6000, now() - interval '20 hours',   'gpt-5-codex');

COMMIT;
