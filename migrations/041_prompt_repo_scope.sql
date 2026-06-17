-- CCT-390 (GH-AGENT-1): repo-scoped prompt selection.
--
-- Extends the core `prompts` table so a prompt can be scoped to a GitHub repo
-- (richelieu-style most-specific-wins): a prompt scoped to `owner/repo` beats
-- one scoped to the whole `owner`, which beats a global (unscoped) prompt.
--
-- `kind` tags the prompt's purpose so the resolver can pick the effective
-- *review* prompt for a repo without colliding with general-purpose prompts.
-- NULL/'general' = a normal prompt; 'review' = a "Review with agent" prompt.
ALTER TABLE prompts
    ADD COLUMN IF NOT EXISTS scope_owner TEXT,
    ADD COLUMN IF NOT EXISTS scope_repo TEXT,
    ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'general';

-- A repo-level scope requires its owner; a bare repo with no owner is
-- meaningless (and would break most-specific-wins resolution).
ALTER TABLE prompts
    ADD CONSTRAINT prompts_repo_requires_owner
    CHECK (scope_repo IS NULL OR scope_owner IS NOT NULL);

-- One effective prompt per (kind, scope) — so resolution is unambiguous.
-- Partial unique indexes because NULLs don't compare equal in a plain UNIQUE.
CREATE UNIQUE INDEX IF NOT EXISTS prompts_kind_global_uniq
    ON prompts (kind) WHERE scope_owner IS NULL AND scope_repo IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS prompts_kind_owner_uniq
    ON prompts (kind, scope_owner) WHERE scope_owner IS NOT NULL AND scope_repo IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS prompts_kind_repo_uniq
    ON prompts (kind, scope_owner, scope_repo) WHERE scope_repo IS NOT NULL;
