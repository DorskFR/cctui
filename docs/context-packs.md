# Portable context packs — adapter-neutral content, adapter-specific targets

A **context pack** is a git-hosted bundle a worker fetches at boot to give a
dispatched agent its documentation environment (see `docs/worker-contract.md`,
*Context pack*). Packing was originally Claude-only: every file mapped to a
`.claude` convention. This document defines the **portable** layer — a pack
declares content once, in an adapter-neutral form, and the worker entrypoint
stages it to the right place for whichever harness the dispatch selects
(`payload.adapter`: `claude` default, or `codex`).

The rule: **content is neutral, targets are per-adapter.** A pack author writes
instructions, prompts, skills, and MCP servers once; the entrypoint owns the
translation. Nothing here changes the Claude path — the Codex targets are
**additive** and gated on `adapter: "codex"`.

## Neutral content → adapter targets

| Neutral pack content | Claude target | Codex target |
| --- | --- | --- |
| `CLAUDE.md` / `AGENTS.md` (home instructions) | `~/CLAUDE.md` | `AGENTS.md` at the dispatch workdir root + `~/.codex/AGENTS.md` |
| `prompts/` (dispatch prompts) | `/opt/context/prompts/` (`TASK_PROMPT_FILE`) | same, plus `~/.codex/prompts/` (custom slash-prompts) |
| `mcp.json` (MCP servers) | merged into `~/.mcp.json` (`mcpServers`) | `~/.codex/config.toml` `[mcp_servers.<name>]` |
| `skills/` | `~/.claude/skills/` | — (not a Codex concept; kept for reference) |
| `rules/` | `~/.claude/rules/` (auto-loaded) | fold into `AGENTS.md` if always-on |
| `docs/` | `~/.claude/docs/` (pull) | `/opt/context/docs/` (referenced by path) |
| `hooks/` | `~/.claude/hooks/` + PreToolUse registration | — (Codex has no PreToolUse hook seam here) |
| `guard-rules.md` | `cctui-guard` rules | `cctui-guard` rules (harness-independent) |

`guard-rules.md` and the guard are harness-independent: the egress fence is
enforced by `cctui-guard-proxy` around whichever agent runs, so a pack's network
policy applies to Codex exactly as to Claude.

### Instructions: `AGENTS.md` with a `CLAUDE.md` fallback

Codex reads project instructions from an `AGENTS.md` walked up from the working
directory, plus a `~/.codex/AGENTS.md` global — it never reads `~/CLAUDE.md`. A
pack MAY ship its own `AGENTS.md` as the Codex-facing instruction source. When it
does not, the entrypoint falls back to the pack's `CLAUDE.md`, so a Claude-shaped
pack still feeds Codex without edits.

### MCP servers: one `mcp.json`, both transports

A pack declares MCP servers once in `mcp.json` using the standard `mcpServers`
map (the same shape as a Claude `.mcp.json`):

```json
{
  "mcpServers": {
    "example-stdio": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-everything"],
      "env": { "EXAMPLE_FLAG": "1" }
    },
    "example-http": {
      "type": "http",
      "url": "https://mcp.example.internal/v1",
      "bearer_token_env_var": "EXAMPLE_MCP_TOKEN"
    }
  }
}
```

- **stdio servers** (`command` + `args` + `env`) are fully portable — they
  translate verbatim to Codex `[mcp_servers.<name>]` and copy into a Claude
  `~/.mcp.json` as-is.
- **streamable-HTTP servers** (`url`) carry both a Claude-facing `headers` and a
  Codex-facing `bearer_token_env_var`. Codex reads `url` + `bearer_token_env_var`;
  Claude reads `url` + `headers`. Include whichever the adapters you dispatch to
  need — the other adapter ignores the extra key.

Codex target (translated by `phase_codex_config`, appended to the managed region
of `~/.codex/config.toml`):

```toml
[mcp_servers.example-stdio]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-everything"]
env = { "EXAMPLE_FLAG" = "1" }

[mcp_servers.example-http]
url = "https://mcp.example.internal/v1"
bearer_token_env_var = "EXAMPLE_MCP_TOKEN"
```

Server names must be **TOML-bare-key safe** (`[A-Za-z0-9_-]`), since they are
emitted unquoted in the table header.

## Entrypoint staging (where this is implemented)

`deploy/worker-entrypoint.sh`:

- `phase_context_pack` — Claude packaging (unchanged) + additive Claude-side
  `mcp.json` → `~/.mcp.json` merge (skipped under the Codex adapter).
- `phase_codex_config` — the `cctui` model provider region, plus (under the Codex
  adapter, when the pack ships `mcp.json`) the translated `[mcp_servers.*]` tables
  appended last inside the managed region — a bare key after a `[table]` would
  bind to it, so the MCP tables must trail every scalar the region sets.
- `phase_codex_pack` — under the Codex adapter: stage `AGENTS.md` (from the pack's
  `AGENTS.md`, else `CLAUDE.md`) to the workdir root + the `~/.codex/AGENTS.md`
  global, and copy `prompts/` into `~/.codex/prompts/`.

A neutral fixture pack, including `mcp.json` and `AGENTS.md`, lives at
`deploy/examples/context-pack/`.

## Security

Nothing here widens the sandbox. The pack is operator-plane config (whoever
controls the pack controls `guard-rules.md`), fetched fail-closed before
lockdown, and `/opt/context` is read-only under Landlock afterwards — identical
to the Claude path. MCP servers a pack declares run inside the same hardened pod
(Landlock + seccomp + `cctui-guard-proxy` egress) and reach only hosts the
active step's guard policy allows.
