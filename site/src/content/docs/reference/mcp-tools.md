---
title: MCP tools for agents
description: The Cctui* tools every session gets, with their parameters.
---

Every session cctui spawns gets a per-session MCP server (`cctui-daemon mcp-agent --session <id>`) and a preamble that explains the tools before the first turn. The relay is fixed to its session, so a session can never act as another. Names and parameters below match `crates/cctui-daemon/src/mcp.rs`.

## CctuiAgent

Spawn a subagent session, follow it while it works, and return its result. Parallel calls run parallel children. Calling it again with `session_id` sends a follow-up to that child and waits again.

| Parameter | Required | Meaning |
|---|---|---|
| `prompt` | yes | The task for the child, or the follow-up message when `session_id` is set. |
| `model` | yes | Model id to run the child on. There is no default. |
| `session_id` | no | A child spawned earlier by this session, for a follow-up. |
| `adapter` | no | Harness for the child: `claude_code`, `codex`, `opencode`, or an ACP agent id (`gemini`, `qwen`, `goose`). Only adapters this session may spawn are accepted. |
| `agent_profile` | no | Named agent profile to run under. |
| `permission_mode` | no | `ask`, `auto` or `yolo`; never more permissive than the parent. |
| `name` | no | Display name for the child in the UI. |
| `budget_usd` | no | Dollar ceiling for the child's own spend; must not exceed what the parent has left. |
| `cwd` | no | Working directory. Defaults to the parent's. |
| `timeout_secs` | no | How long to wait for the child before giving up. |

## CctuiAgentArchive

Archive a child this session spawned.

| Parameter | Required | Meaning |
|---|---|---|
| `session_id` | yes | The child to archive, as returned by `CctuiAgent`. |

## CctuiUsage

Report the rate limits and budget that apply to this session: windows, remaining headroom, the dollar cap and what is left.

| Parameter | Required | Meaning |
|---|---|---|
| `model` | no | Ask about one model instead of this session's current one. |

## CctuiPeers

List the sessions this one may talk to (parent, children, room members, `remote:` cctuiverse links), with their relation and state. No parameters.

## CctuiSend

Send a message to another session, on any machine and under any harness. The reply, if any, arrives as a normal turn.

| Parameter | Required | Meaning |
|---|---|---|
| `session_id` | yes | The peer, as reported by `CctuiPeers`. |
| `message` | yes | What to say, written for another agent. |

## CctuiHistory

Read another session's conversation.

| Parameter | Required | Meaning |
|---|---|---|
| `session_id` | yes | The peer whose history to read. |
| `before` | no | Return events older than this seq (page backwards). |
| `after` | no | Return events newer than this seq, oldest first (a delta). |
| `limit` | no | Events to read. Default 200, max 1000. |
| `roles` | no | Keep only these roles, e.g. `["user", "assistant"]`. |
| `format` | no | `markdown` (default) or `json`. |

## CctuiRoom

Broadcast to the room a human put this session in, or read it. A session is in at most one room.

| Parameter | Required | Meaning |
|---|---|---|
| `action` | yes | `post` to say something, `peek` to read the timeline, `members` to see who is in it. |
| `message` | for `post` | What to post. |
| `room_id` | no | Rarely needed; the session's room is implied. |

## CctuiSpeak

Post a voice note into this session's conversation; the text is synthesised and shown with a transcript.

| Parameter | Required | Meaning |
|---|---|---|
| `text` | yes | What to say, as plain spoken prose. |
| `voice` | no | Voice id; omit for the instance default. |

## CctuiContext

Keep durable notes for future sessions, and attach existing ones to this session.

| Parameter | Required | Meaning |
|---|---|---|
| `action` | yes | `list`, `show`, `save` or `attach`. |
| `kind` | no | `rule`, `memory` or `prompt`. Filters `list`; picks the item for `show` (default `memory`). |
| `name` | for `show`, `save` | Slug, 1 to 64 chars of `a-z`, `0-9` and dashes. |
| `title` | no | Human title, max 120 chars. |
| `body` | for `save` | Markdown. A rule is capped at 4 KiB. |
| `match` | no | When the item applies: keys are AND-ed (adapter, directory, label), values within a key OR-ed. |
| `mode` | no | For `save`: create (default), replace, or append. |
| `names` | for `attach` | Exact memory names to attach. |

## User actions

A "needs you" checklist the user ticks off in the UI.

### CctuiUserActionAdd

| Parameter | Required | Meaning |
|---|---|---|
| `title` | yes | One imperative line addressed to the user. |
| `detail` | no | Markdown: why, the exact command, links. |
| `kind` | no | `action` (the user must do something), `input` (the user must tell you something) or `decision` (the user must choose). Default `action`. |
| `blocking` | no | `true` when the main line of work cannot continue without it. |

### CctuiUserActionTick

| Parameter | Required | Meaning |
|---|---|---|
| `id` | yes | The item id, as returned by `CctuiUserActionAdd`. |
| `status` | yes | `done` when you saw it happen, `dropped` when it is no longer needed. |
| `note` | no | Short outcome. |
