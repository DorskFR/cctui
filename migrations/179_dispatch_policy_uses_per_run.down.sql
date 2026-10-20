DROP INDEX IF EXISTS dispatch_policy_uses_by_session;
DELETE FROM dispatch_policy_uses a
    USING dispatch_policy_uses b
    WHERE a.session_id = b.session_id AND a.id > b.id;
ALTER TABLE dispatch_policy_uses DROP CONSTRAINT IF EXISTS dispatch_policy_uses_pkey;
ALTER TABLE dispatch_policy_uses DROP COLUMN IF EXISTS id;
ALTER TABLE dispatch_policy_uses ADD PRIMARY KEY (session_id);
