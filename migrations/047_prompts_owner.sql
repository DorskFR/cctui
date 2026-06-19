-- CCT-418: owner-scope prompts. `scope_owner`/`scope_repo` describe the GitHub
-- repo a prompt *applies to* (CCT-390) — they are not an access-control owner.
-- Add a real `user_id` owner so list/get/delete/resolve only surface a caller's
-- own prompts (admin sees all). Pre-existing rows have NULL owner (legacy /
-- admin-owned) and are admin-only-visible.
ALTER TABLE prompts
    ADD COLUMN IF NOT EXISTS user_id UUID REFERENCES users(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS prompts_user_id_idx ON prompts (user_id);
