---
title: Comparison
description: Where cctui stands next to other tools that monitor, drive or orchestrate coding agents.
---

The landscape, as surveyed in October 2026. `yes`, `no`, `~` partial, `?` not stated publicly. This is kept factual; when a competitor ships something, the row changes.

## Where cctui stands

- **Closest twins**: Multica, Paseo, HAPI, bb, Happy and T3 Code. All pair a per-machine daemon with a web or mobile client.
- **First-party vendor products** (Claude Code Remote Control and Projects, Codex remote) cover much of cctui for a single vendor, hosted, not self-hostable.
- **What nobody else combines**: MIT and fully self-hosted; one UI over Claude Code, Codex, opencode and ACP with real permission-prompt answering; ephemeral Kubernetes workers behind a guard proxy; nested subagents and rooms across harnesses; per-account rate-limit tracking with dollar caps.
- **Where cctui trails**: native iOS and Android apps with push, an end-to-end-encrypted relay needing no inbound access, diff and PR review inside the session (cctui has it as a mod), task boards, and evidence-backed review of a finished change.

## Head-to-head

| | cctui | Multica | Paseo | Vendor remote control | Happy | HAPI | bb | T3 Code |
|---|---|---|---|---|---|---|---|---|
| Shape | server + daemon + web/TUI | server + daemon + web/desktop | daemon + apps | built into the CLI + app | CLI + relay + apps | hub + runner + apps | server + hosts + web | server + apps |
| License | MIT | ~ source-available | Apache-2.0 | proprietary | MIT | AGPL-3.0 | MIT | MIT |
| Self-hostable | yes | yes | yes | ~ runners only | yes | yes | yes | ~ |
| Claude Code / Codex | yes / yes | yes / yes | yes / yes | yes / no | yes / yes | yes / yes | yes / yes | yes / yes |
| opencode / ACP agents | yes / yes | yes / yes | yes / yes | no / no | no / no | yes / no | yes / ~ | yes / ~ |
| Live transcript + send input | yes | yes | yes | yes | yes | yes | yes | yes |
| Answer permission prompts | yes | ? | yes | yes | yes | yes | yes | yes |
| Spawn on a remote machine | yes | yes | yes | yes | ? | yes | yes | yes |
| Ephemeral workers (k8s) | yes | ? | ~ | ~ cloud sessions | no | ? | ~ | no |
| Mobile | web | ~ iOS (build it) | native | native | native | native | ~ early | native |
| Push notifications | ~ ntfy | ? | ? | yes | yes | yes | yes | yes |
| Voice | live + notes | ? | yes | ? | yes | yes | ? | ~ |
| Nested subagents | yes | ? | yes | yes | ~ | ? | yes | ? |
| Cross-session messaging | rooms + peers | ? | ~ handoff | yes | ? | yes | ? | ? |
| Transcript search and archive | yes | yes | ? | ~ | ? | ? | ? | ? |
| Usage and rate-limit tracking | 5h/7d + per model | cost | ? | ~ | no | ? | ~ | cost |
| Dollar caps | yes, per session and subagent | ? | no | no | no | no | no | no |
| Diff / PR review | ~ mod | yes | yes | ~ | ? | ? | ? | yes |
| Task board | no | yes | no | no | no | no | no | no |

## Categories around cctui

- **Self-hosted fleet control planes** (the same category): Multica, Paseo, HAPI, bb, AgentsMesh, Agent Deck, Jean, mission-control.
- **Remote and mobile control** for one machine: Happy, T3 Code, AionUi, CloudCLI, VibeTunnel, Yep Anywhere.
- **Desktop parallel-agent apps** on one machine, usually one worktree per agent: Orca, Vibe Kanban, cmux, Superset, Emdash, Nimbalyst.
- **Hosted background-agent platforms** in vendor sandboxes: Tembo, Replicas, Devin, Factory, Amp, OpenHands.
- **Read-only usage and transcript tools**: ccusage, Claude-Code-Usage-Monitor, claude-code-log, Langfuse and OTel exports. None can send input or answer prompts.

The full survey with every product, licence and note lives in `docs/competitors.md` in the repository.
