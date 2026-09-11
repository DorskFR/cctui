\timing on
\set sel 'SELECT s.id, s.parent_id, s.machine_id, s.working_dir, s.status, s.registered_at, s.last_heartbeat, s.metadata, s.adapter_id, COALESCE(m.display_name, m.name) AS resolved_machine_name, m.hue AS resolved_machine_hue, m.kind AS resolved_machine_kind FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid'

\echo '=== A. main query: single COMMON term (error) ==='
EXPLAIN (ANALYZE, BUFFERS) :sel WHERE (TRUE) AND ((s.id ILIKE '%error%' OR COALESCE(s.session_name, '') ILIKE '%error%' OR s.working_dir ILIKE '%error%' OR EXISTS (SELECT 1 FROM stream_events e WHERE e.session_id = s.id AND left(e.search_text, 8192) ILIKE '%error%'))) AND (NULL::uuid IS NULL OR m.user_id = NULL::uuid) ORDER BY s.registered_at DESC LIMIT 100 OFFSET 0;

\echo '=== B. main query: single RARE term (zygomorphic) ==='
EXPLAIN (ANALYZE, BUFFERS) :sel WHERE (TRUE) AND ((s.id ILIKE '%zygomorphic%' OR COALESCE(s.session_name, '') ILIKE '%zygomorphic%' OR s.working_dir ILIKE '%zygomorphic%' OR EXISTS (SELECT 1 FROM stream_events e WHERE e.session_id = s.id AND left(e.search_text, 8192) ILIKE '%zygomorphic%'))) AND (NULL::uuid IS NULL OR m.user_id = NULL::uuid) ORDER BY s.registered_at DESC LIMIT 100 OFFSET 0;

\echo '=== C. main query: TWO terms (quokka handler) -> two independent EXISTS ==='
EXPLAIN (ANALYZE, BUFFERS) :sel WHERE (TRUE) AND (((s.id ILIKE '%quokka%' OR COALESCE(s.session_name, '') ILIKE '%quokka%' OR s.working_dir ILIKE '%quokka%' OR EXISTS (SELECT 1 FROM stream_events e WHERE e.session_id = s.id AND left(e.search_text, 8192) ILIKE '%quokka%')) AND (s.id ILIKE '%handler%' OR COALESCE(s.session_name, '') ILIKE '%handler%' OR s.working_dir ILIKE '%handler%' OR EXISTS (SELECT 1 FROM stream_events e WHERE e.session_id = s.id AND left(e.search_text, 8192) ILIKE '%handler%')))) AND (NULL::uuid IS NULL OR m.user_id = NULL::uuid) ORDER BY s.registered_at DESC LIMIT 100 OFFSET 0;

\echo '=== D. main query: NEGATED term (NOT zygomorphic) ==='
EXPLAIN (ANALYZE, BUFFERS) :sel WHERE (TRUE) AND ((NOT (s.id ILIKE '%zygomorphic%' OR COALESCE(s.session_name, '') ILIKE '%zygomorphic%' OR s.working_dir ILIKE '%zygomorphic%' OR EXISTS (SELECT 1 FROM stream_events e WHERE e.session_id = s.id AND left(e.search_text, 8192) ILIKE '%zygomorphic%')))) AND (NULL::uuid IS NULL OR m.user_id = NULL::uuid) ORDER BY s.registered_at DESC LIMIT 100 OFFSET 0;

\echo '=== E. main query: SHORT sub-trigram term (ok) ==='
EXPLAIN (ANALYZE, BUFFERS) :sel WHERE (TRUE) AND ((s.id ILIKE '%ok%' OR COALESCE(s.session_name, '') ILIKE '%ok%' OR s.working_dir ILIKE '%ok%' OR EXISTS (SELECT 1 FROM stream_events e WHERE e.session_id = s.id AND left(e.search_text, 8192) ILIKE '%ok%'))) AND (NULL::uuid IS NULL OR m.user_id = NULL::uuid) ORDER BY s.registered_at DESC LIMIT 100 OFFSET 0;

\echo '=== F. fielded filter tag:tag7 ==='
EXPLAIN (ANALYZE, BUFFERS) :sel WHERE (TRUE) AND (EXISTS (SELECT 1 FROM session_labels sl JOIN labels l ON l.id = sl.label_id WHERE sl.session_id = s.id AND lower(l.name) = lower('tag7'))) AND (NULL::uuid IS NULL OR m.user_id = NULL::uuid) ORDER BY s.registered_at DESC LIMIT 100 OFFSET 0;

\echo '=== G. empty-q archive browse (offset 0) ==='
EXPLAIN (ANALYZE, BUFFERS) :sel WHERE s.status = 'archived' AND (NULL::uuid IS NULL OR m.user_id = NULL::uuid) ORDER BY s.registered_at DESC LIMIT 100 OFFSET 0;

\echo '=== H. empty-q archive browse (deep offset 2000) ==='
EXPLAIN (ANALYZE, BUFFERS) :sel WHERE s.status = 'archived' AND (NULL::uuid IS NULL OR m.user_id = NULL::uuid) ORDER BY s.registered_at DESC LIMIT 100 OFFSET 2000;

\echo '=== I. SNIPPET pass for the common term (100 ids) ==='
EXPLAIN (ANALYZE, BUFFERS) SELECT DISTINCT ON (session_id) session_id, left(search_text, 8192), id FROM stream_events WHERE session_id = ANY(ARRAY(SELECT id FROM sessions WHERE id LIKE 'bench-sess-%' ORDER BY registered_at DESC LIMIT 100)) AND (left(search_text, 8192) ILIKE '%error%') ORDER BY session_id, created_at DESC;

\echo '=== J. SNIPPET pass at the 500-row limit ==='
EXPLAIN (ANALYZE, BUFFERS) SELECT DISTINCT ON (session_id) session_id, left(search_text, 8192), id FROM stream_events WHERE session_id = ANY(ARRAY(SELECT id FROM sessions WHERE id LIKE 'bench-sess-%' ORDER BY registered_at DESC LIMIT 500)) AND (left(search_text, 8192) ILIKE '%error%') ORDER BY session_id, created_at DESC;
