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
- `docs/`, `skills/`, `guard-rules.md` — shared with the Claude packaging.
- `mcp.json` — adapter-neutral MCP servers. For Codex they are translated into
  `~/.codex/config.toml` `[mcp_servers.<name>]` tables; for Claude they merge into
  `~/.mcp.json`.

See `docs/context-packs.md` and `docs/worker-contract.md` in the cctui repo for
the full portable context-pack contract.
