-- Account hook rules, delivered on every spawn the account launches.
ALTER TABLE public.account_tool_policies
    ADD COLUMN IF NOT EXISTS hooks jsonb NOT NULL DEFAULT '[]'::jsonb;
-- What a launch carried: {"rules": [...], "status": "applied"|"unsupported"}.
-- NULL when the launch carried no rules.
ALTER TABLE public.machine_launches
    ADD COLUMN IF NOT EXISTS hook_policy jsonb;
