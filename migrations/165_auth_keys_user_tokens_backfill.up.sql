-- Re-run migration 044's user_tokens backfill.
--
-- Auth resolves `auth_keys` first and falls back to `user_tokens`. Every writer
-- now creates both rows, but `mint_user_key` wrote them in two statements
-- rather than one transaction, so a failure between them could leave a
-- `user_tokens` row with no `auth_keys` mirror — a credential reachable only
-- through the fallback, and therefore missed by a revoke that keys off
-- `auth_keys`. Idempotent (ON CONFLICT on the unique key_hash): closes that
-- drift so `auth_keys` can become the single source of truth.

INSERT INTO auth_keys
    (id, user_id, key_hash, key_preview, label, kind, expires_at, revoked_at, created_at)
    SELECT
        gen_random_uuid(),
        t.user_id, t.token_hash, t.token_preview, t.label, 'user',
        t.expires_at, t.revoked_at, t.created_at
    FROM user_tokens t
    JOIN users u ON u.id = t.user_id
    ON CONFLICT (key_hash) DO NOTHING;

-- Grant each freshly-mirrored key the owner's ceiling, so it keeps doing
-- exactly what it does through the fallback today (no narrowing, no widening).
INSERT INTO key_acls (key_id, scope)
    SELECT k.id, ua.scope
    FROM auth_keys k
    JOIN user_acls ua ON ua.user_id = k.user_id
    WHERE k.kind = 'user'
    ON CONFLICT DO NOTHING;
