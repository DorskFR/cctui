# Worker instructions (context pack example — Codex target)

This is the adapter-neutral instruction file for the dispatched agent, the Codex
counterpart of this pack's `CLAUDE.md`. When a dispatch selects `adapter: "codex"`,
the worker entrypoint stages this file to `AGENTS.md` at the dispatch workdir root
(where `codex exec` reads it) and to the `~/.codex/AGENTS.md` global. If a pack
ships no `AGENTS.md`, the entrypoint falls back to its `CLAUDE.md` so a
Claude-shaped pack still feeds Codex.

This fixture is intentionally NEUTRAL. Replace it with your own organization's
instructions when you build a derived pack.

## What lives here

- `prompts/` — dispatch prompts. For Codex they are also staged to
  `~/.codex/prompts/` as custom slash-prompts; `TASK_PROMPT_FILE` still resolves
  under `/opt/context/prompts/`.
- `rules/`, `docs/`, `schemas/`, `skills/`, `guard-rules.md`, `capabilities.toml`,
  `pack.toml` —
  shared with the Claude packaging. Always-on `rules/` are best folded into this
  `AGENTS.md` for Codex (Codex has no `~/.claude/rules/` auto-load).
- `mcp.json` — adapter-neutral MCP servers. For Codex they are translated into
  `~/.codex/config.toml` `[mcp_servers.<name>]` tables; for Claude they merge into
  `~/.mcp.json`.

## When a command is denied

A guarded dispatch runs in numbered steps, and each step grants a set of
capabilities (`git:read`, `remote:write`, …). If a command fails with
`Permission denied`, run `cctui-guard why` to see the reason, the current step
and the capabilities it grants. Do not retry the command another way: move to
the step that grants the capability, or report that you are blocked.

See `docs/context-packs.md` and `docs/worker-contract.md` in the cctui repo for
the full portable context-pack contract.
