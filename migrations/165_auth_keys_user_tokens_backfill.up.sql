-- Mirror any user_tokens row that has no auth_keys row. Only the inserted rows
-- get the owner's ceiling as grant, which is what the legacy fallback grants
-- them today; existing scoped keys are left untouched.
WITH mirrored AS (
    INSERT INTO auth_keys
        (id, user_id, key_hash, key_preview, label, kind, expires_at, revoked_at, created_at)
        SELECT
            gen_random_uuid(),
            t.user_id, t.token_hash, t.token_preview, t.label, 'user',
            t.expires_at, t.revoked_at, t.created_at
        FROM user_tokens t
        JOIN users u ON u.id = t.user_id
        ON CONFLICT (key_hash) DO NOTHING
        RETURNING id, user_id
)
INSERT INTO key_acls (key_id, scope)
    SELECT m.id, ua.scope
    FROM mirrored m
    JOIN user_acls ua ON ua.user_id = m.user_id
    ON CONFLICT DO NOTHING;
