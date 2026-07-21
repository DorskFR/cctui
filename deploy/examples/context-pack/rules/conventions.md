# Always-on conventions (context pack example)

Guidance that applies to **every** dispatched task regardless of the repo. It is
copied to `~/.claude/rules/`, which Claude Code auto-loads as instructions on
every task — so keep it short and universal. This is the **push** seam: rules
are always in context. On-demand reference (linked from a prompt when needed)
belongs in `docs/`, not here.

This fixture is NEUTRAL — replace every line with your organization's real
conventions when you build a derived pack.

## Conventions

- Prefer the repo's existing helpers and patterns over new abstractions; search
  the generated helper index (`docs/helper-index.md`) before introducing one.
- Make the smallest change that satisfies the ratified Acceptance; do not
  gold-plate or refactor unrelated code.
- Every claim of "done" is backed by evidence (a command + its output, a diff, a
  screenshot) — never a bare assertion.
- Never write a real secret to disk or logs; the worker holds placeholders only.
- Match the result-callback envelope exactly (see `schemas/result.json`); callers
  branch on the structured fields, never on prose.
