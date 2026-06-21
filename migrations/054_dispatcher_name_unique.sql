-- CCT-451: dispatcher names must be globally unique among live dispatchers.
--
-- Previously uniqueness was scoped per (user_id, name) (033's
-- dispatchers_user_name_live), so a re-enrollment under a DIFFERENT principal
-- silently created a SECOND dispatcher with the same name. Resolution
-- (resolve_dispatcher) is owner-scoped, so the dispatch caller kept resolving
-- its own (now offline) row while the live WS connection was on the other
-- principal's row → every dispatch failed with "dispatcher '<name>' is offline"
-- (502) and no sessions were produced. Make the NAME the unique key so a shadow
-- enrollment is rejected outright instead of silently shadowing the live one.
--
-- Requires no live (deleted_at IS NULL) duplicate names. The prod fleet was
-- consolidated to a single row before this ships; if a duplicate exists, the
-- index build fails and the operator must consolidate first (the correct
-- action — two same-named dispatchers is the bug this prevents).
DROP INDEX IF EXISTS dispatchers_user_name_live;

CREATE UNIQUE INDEX IF NOT EXISTS dispatchers_name_live
    ON dispatchers (name)
    WHERE deleted_at IS NULL;
