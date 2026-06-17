-- GH-PKG-2: initial GitHub-integration schema.
--
-- This migration runs with `search_path = github` (see `cctui_github::migrate`),
-- so every unqualified object below — including sqlx's own `_sqlx_migrations`
-- bookkeeping table — is created in the dedicated `github` schema, independent
-- of core's migration history.
--
-- Invariant (docs/github-integration.md §7.2): FKs may point *from* github.*
-- *into* core, but core never references github.*. Because a FK constraint
-- lives on the referencing table, `DROP SCHEMA github CASCADE` removes every
-- GitHub table and its outbound constraints without touching core.

-- The connector holds the (encrypted) GitHub credential + config for a user.
-- `user_id` is a one-directional FK into core's `users` table: dropping the
-- github schema drops this constraint, leaving `users` untouched.
CREATE TABLE connectors (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID        NOT NULL REFERENCES public.users (id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX connectors_user_id_idx ON connectors (user_id);
