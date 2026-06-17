# Worker home instructions (context pack example)

This is the home-level `CLAUDE.md` for the dispatched agent — read on every task
regardless of the repo it works in. It is copied to `/home/worker/CLAUDE.md` at
boot by the worker entrypoint's context-pack phase.

This fixture is intentionally NEUTRAL: it documents the layout and shows how a
real pack wires prompts, docs, skills, and guard rules together. Replace it with
your own organization's instructions when you build a derived pack.

## What lives here

- `prompts/` — dispatch prompt files; `TASK_PROMPT_FILE` resolves under here.
- `docs/` — reference docs exposed to the agent.
- `skills/` — skill directories (each with a `SKILL.md`).
- `projects/` — per-repo `CLAUDE.md` overlays.
- `style/` — output styles.
- `guard-rules.md` — tool-set + network-set definitions for `cctui-guard`.

See `docs/worker-contract.md` in the cctui repo for the full contract.
