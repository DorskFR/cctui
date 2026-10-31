# Competitors

Landscape of tools that monitor, drive or orchestrate coding agents, as of 2026-10-14.
`✅` yes, `❌` no, `~` partial, `?` unknown or not stated publicly. Stars are GitHub
stars on that date. Harnesses: CC Claude Code, Cx Codex, oc opencode, Gm Gemini /
Antigravity.

When a new one shows up: add a row to its category table, and a column to the
[head-to-head](#head-to-head) matrix only if it is a direct competitor.

## Where cctui stands

- **Closest twins**: [Multica](#self-hosted-fleet-control-planes) (daemon per machine +
  server + board, 26 harnesses), Paseo, HAPI, bb, Happy and T3 Code. All pair a
  per-machine daemon with a web/mobile client.
- **First party is the biggest threat**: Anthropic Remote Control + Projects
  (device cards, spawn from phone, permission prompts, cross-session messaging) and
  Codex remote cover most of cctui for a single vendor, hosted, not self-hostable.
- **Nobody else combines**: MIT + fully self-hosted, one UI over CC/Cx/oc/ACP with
  real permission-prompt answering, ephemeral k8s/Docker workers, nested subagents
  and rooms across harnesses, and per-account rate-limit tracking with dollar caps.
- **Where cctui trails**: native iOS/Android apps with push, an E2E-encrypted relay
  needing no inbound access (Happy, Paseo), adopting sessions started outside the
  tool (Yep Anywhere, CloudCLI), diff/PR review in the session, task boards
  (Multica, Vibe Kanban) and evidence-backed review of a finished change (Blume,
  Antigravity walkthroughs).
- **Churn is high**: Vibe Kanban's company shut down, Terragon shut down, Crystal
  was deprecated for Nimbalyst, Codegen went to ClickUp, Ona to OpenAI, Coder Tasks
  was retired, Omnara pivoted from mobile remote to a managed platform.

## Head-to-head

| Feature                        | cctui                  | Multica                      | Paseo               | Anthropic RC / Projects | Happy            | HAPI           | bb          | T3 Code        | Agent Deck      | Blume                |
| ------------------------------ | ---------------------- | ---------------------------- | ------------------- | ----------------------- | ---------------- | -------------- | ----------- | -------------- | --------------- | -------------------- |
| Shape                          | server + web + daemon  | server + daemon + web/desktop | daemon + apps      | built into CC + app     | CLI + relay + apps | hub + runner + apps | server + hosts + web | server + apps | TUI + web     | desktop + plugin     |
| License                        | MIT                    | ~ source-available           | Apache-2.0          | ❌                      | MIT              | AGPL-3.0       | MIT         | MIT            | MIT             | ?                    |
| Stars                          | –                      | 52.7k                        | 20.8k               | –                       | 24.1k            | 5.2k           | 4.3k        | 27.4k          | 1.1k            | – (waitlist)         |
| Self-hostable                  | ✅                     | ✅                           | ✅                  | ~ runners only          | ✅               | ✅             | ✅          | ~              | ✅              | n/a                  |
| Claude Code / Codex            | ✅ / ✅                | ✅ / ✅                      | ✅ / ✅             | ✅ / ❌                 | ✅ / ✅          | ✅ / ✅        | ✅ / ✅     | ✅ / ✅        | ✅ / ~          | ✅ / ✅              |
| opencode / Gemini (ACP)        | ✅ / ✅                | ✅ / ✅                      | ✅ / ✅             | ❌ / ❌                 | ❌ / ❌          | ✅ / ❌        | ✅ / ~      | ✅ / ~         | ~ / ✅          | ? / ~                |
| Live transcript + send input   | ✅                     | ✅                           | ✅                  | ✅                      | ✅               | ✅             | ✅          | ✅             | ✅              | ✅ / ?               |
| Answer permission prompts      | ✅                     | ?                            | ✅                  | ✅                      | ✅               | ✅             | ✅          | ✅             | ~               | ?                    |
| Spawn on a remote machine      | ✅                     | ✅                           | ✅                  | ✅                      | ?                | ✅             | ✅          | ✅             | ~ SSH           | ?                    |
| Multi-machine fleet            | ✅                     | ✅                           | ✅                  | ✅                      | ✅               | ✅             | ✅          | ✅             | ✅ SSH          | ?                    |
| Ephemeral workers (k8s/Docker) | ✅                     | ?                            | ~                   | ~ cloud sessions        | ❌               | ?              | ~ Modal     | ❌             | ✅ Docker       | ❌                   |
| Mobile                         | ✅ web                 | ~ iOS (build it)             | ✅ native           | ✅ native               | ✅ native        | ✅ native      | ~ early     | ✅ native      | ✅ web          | ?                    |
| Push notifications             | ~ in-app               | ?                            | ?                   | ✅                      | ✅               | ✅             | ✅          | ✅             | ✅              | ?                    |
| Voice                          | ✅ notes               | ?                            | ✅                  | ?                       | ✅               | ✅             | ?           | ~              | ❌              | ?                    |
| Nested subagents               | ✅                     | ?                            | ✅                  | ✅                      | ~                | ?              | ✅          | ?              | ~               | ?                    |
| Cross-session messaging        | ✅ rooms               | ?                            | ~ handoff           | ✅                      | ?                | ✅             | ?           | ?              | ~ chat bots     | ~                    |
| Transcript search / archive    | ✅                     | ✅                           | ?                   | ~                       | ?                | ?              | ?           | ?              | ?               | ?                    |
| Usage / rate-limit tracking    | ✅                     | ✅ cost                      | ?                   | ~                       | ❌               | ?              | ~           | ✅ cost        | ✅ 5h/7d        | ~                    |
| Dollar caps                    | ✅                     | ?                            | ❌                  | ❌                      | ❌               | ❌             | ❌          | ❌             | ~ budgets       | ?                    |
| Diff / PR review               | ~ GitHub plugin        | ✅                           | ✅                  | ~                       | ?                | ?              | ?           | ✅             | ?               | ✅                   |
| Task board                     | ❌                     | ✅                           | ❌                  | ❌                      | ❌               | ❌             | ❌          | ❌             | ❌              | ✅                   |
| Evidence-backed review         | ❌                     | ~                            | ❌                  | ❌                      | ❌               | ❌             | ❌          | ❌             | ❌              | ✅                   |

## Self-hosted fleet control planes

Your machines, a daemon or runner on each, one central UI. The closest category.

| Product                                                      | License            | Stars  | Harnesses              | Notes                                                                 |
| ------------------------------------------------------------ | ------------------ | ------ | ---------------------- | --------------------------------------------------------------------- |
| [Multica](https://github.com/multica-ai/multica)             | ~ custom (Apache + hosting restriction) | 52.7k | 26 incl. CC Cx oc | Issue board, steer mid-run, decision inbox, cost per run, Helm, roles |
| [Paseo](https://github.com/getpaseo/paseo)                   | Apache-2.0         | 20.8k  | CC Cx oc Gm Copilot Pi | E2E relay, native apps, voice, agent handoff, phone-rendered plugins  |
| [HAPI](https://github.com/tiann/hapi)                        | AGPL-3.0           | 5.2k   | CC Cx oc               | Hub + runner per machine, cross-host peer tools, voice, Telegram      |
| [bb](https://github.com/get-bb/bb)                           | MIT                | 4.3k   | CC Cx oc ~Gm           | Hosts enroll with a server, nested threads, push, Modal plugin        |
| [AgentsMesh](https://github.com/AgentsMesh/AgentsMesh)       | ~ BSL-1.1          | 2.4k   | CC Cx Gm oc Aider      | Self-hosted Go runner fleet, sandboxed pods, @mention channels, iOS   |
| [Agent Deck](https://github.com/asheshgoplani/agent-deck)    | MIT                | 1.1k   | CC Gm, ~Cx ~oc         | SSH remotes, Docker sandboxes, $ budgets, 5h/7d quota, Telegram/Slack |
| [Jean](https://github.com/coollabsio/jean)                   | Apache-2.0         | 1.3k   | CC Cx oc Cursor Pi …   | Headless `jean-server`, several servers in one sidebar, usage         |
| [mission-control](https://github.com/builderz-labs/mission-control) | MIT         | 6.3k   | CC Cx                  | Kanban, review gate, cost tracking, roles (alpha)                     |
| Paperclip                                                    | MIT                | 70k    | multi                  | Agents as a "company", per-agent budgets that pause at 100%           |
| AgentOS                                                      | MIT                | 8      | CC Cx Gm oc            | Linked machines, agent network, orchestrator; feature set closest to cctui |
| [Continuum](https://continuumcode.ai)                        | ?                  | –      | CC Cx Cursor oc Grok   | Register any machine, drive from web/desktop/iPhone; free             |
| [AQ](https://aq.dev)                                         | ❌                 | –      | real CLIs in tmux      | Your VMs streamed to browser, multiplayer takeover; $50/user          |
| Omnigent, Open-Inspect                                       | OSS                | –      | multi                  | k8s / many sandbox backends (early)                                   |

## First-party vendor products

Single-vendor, hosted, not self-hostable. They are absorbing cctui's single-harness features.

| Product                                 | Agents     | Notes                                                                                     |
| --------------------------------------- | ---------- | ----------------------------------------------------------------------------------------- |
| Claude Code Remote Control              | CC         | GA Aug 2026. Device cards, spawn from phone, permissions, push, up to 32 sessions/server   |
| Claude Code Projects / agent view       | CC         | Parallel cloud or local threads, cross-session messaging across machines; Pro/Max only     |
| Claude Code cloud sessions              | CC         | GA Sep 2026, `claude --cloud`, self-hosted runners (beta, Team/Enterprise)                 |
| Claude desktop Code tab                 | CC         | Multi-session with worktrees                                                               |
| Codex cloud / Codex remote / Codex app  | Cx         | Steer from phone, Handoff between machines, review inbox, worktrees; Mac/Windows hosts    |
| GitHub Copilot agent / Agent HQ         | Copilot CC Cx | Runs in Actions (59 min cap), mobile approvals via Copilot CLI remote; $10–39/seat     |
| Cursor 3 Agents Window / cloud agents   | Cursor     | Local, worktree, cloud VM or SSH; self-hosted workers via Helm (Teams+)                    |
| Google Antigravity 2.0 / Jules          | Gm         | Multi-agent desktop with walkthrough evidence; Jules async cloud tasks                     |
| Warp Agent Management / Oz              | Warp CC Cx Gm oc | Agent panel, remote control, Oz cloud agents that spawn CC/Cx subagents             |

## Remote and mobile control

Drive agents running on your own machine from a phone or browser.

| Product                                                 | License          | Stars | Harnesses        | Notes                                                              |
| ------------------------------------------------------- | ---------------- | ----- | ---------------- | ------------------------------------------------------------------ |
| [Happy](https://happy.engineering)                      | MIT              | 24.1k | CC Cx            | E2E-encrypted relay, native apps, voice, multiplayer sessions      |
| [T3 Code](https://github.com/pingdotgg/t3code)          | MIT              | 27.4k | CC Cx oc …       | Paired machines, native apps, checkpoint rollback, cost tracking   |
| AionUi                                                  | Apache-2.0       | 33.5k | CC Cx Gm oc      | Desktop + web UI + chat bots, team mode                            |
| [CloudCLI / Claude Code UI](https://github.com/siteboon/claudecodeui) | AGPL-3.0 | 14.1k | CC Cx Cursor oc | Self-hosted web UI, hosted containers from €7/mo                |
| [VibeTunnel](https://github.com/amantus-ai/vibetunnel)  | MIT              | 4.7k  | any terminal     | Terminal in the browser                                            |
| [Yep Anywhere](https://github.com/kzahel/yepanywhere)   | MIT              | 536   | CC Cx            | Adopts existing sessions, fork conversations, diff line comments   |
| [Avibe](https://github.com/cyhhao/vibe-remote) (ex Vibe Remote) | MIT      | 625   | CC Cx oc         | Chat-app bridges, model gateway with subscription failover        |
| Vicoa                                                   | AGPL-3.0         | 516   | CC Cx Gm oc      | CLI + desktop/web/mobile                                           |
| Termly                                                  | ~ CLI MIT        | 316   | CC Cx Gm oc      | Mobile apps, voice                                                 |
| [AgentsRoom](https://agentsroom.dev)                    | ❌ paid          | –     | 14 providers     | Switch provider mid-conversation, iOS Live Activities              |
| CodeAgent Mobile                                        | ❌               | –     | CC Cx oc         | Jira/Linear tickets to sessions, voice                             |
| CosmoRemote                                             | ~                | –     | CC Cx            | Streams iOS simulator / Android emulator to the phone              |
| Tactic Remote, Kisuke, Kibbler, Remote Codetrol         | ❌               | –     | CC Cx …          | Mobile companions for a desktop host                               |
| Omnara (legacy mobile)                                  | Apache-2.0       | –     | CC Cx            | Pivoted to a managed agent platform                                |
| Claude-Code-Remote, ccremote, Telegram bridges          | MIT              | ≤1.3k | CC               | Email / Discord / Telegram bridges; mostly stalled                 |

## Desktop parallel-agent apps

Several agents side by side on one machine, usually one git worktree each.

| Product                                                   | License           | Stars | Pricing           | Notes                                                        |
| --------------------------------------------------------- | ----------------- | ----- | ----------------- | ------------------------------------------------------------ |
| [Orca](https://github.com/stablyai/orca)                  | MIT               | 91.8k | Free              | 30+ harnesses, phone companion, line comments back to agent  |
| [Vibe Kanban](https://github.com/BloopAI/vibe-kanban)     | Apache-2.0        | 28.4k | Free              | Board per task; company shut down, community-maintained      |
| [cmux](https://github.com/manaflow-ai/cmux)               | GPL-3 / BSL       | 28.2k | Free + paid       | Native macOS terminal, `cmux ssh`, cloud VMs (early)         |
| [opcode](https://github.com/winfunc/opcode) (ex Claudia)  | AGPL-3.0          | 22.4k | Free              | CC only, cost dashboard; effectively dormant                 |
| [Superset](https://superset.sh)                           | ~ Elastic 2.0     | 15.1k | Free / $20 Pro    | Remote machines and mobile on Pro                            |
| [Claude Squad](https://github.com/smtg-ai/claude-squad)   | AGPL-3.0          | 8.6k  | Free              | TUI                                                          |
| [Emdash](https://github.com/generalaction/emdash)         | Apache-2.0        | 6.0k  | Free              | 9+ harnesses, diff/PR/CI/merge, SSH                          |
| [Xum](https://github.com/coder/xum) (ex Mux)              | AGPL-3.0          | 2.1k  | Free              | Own agent loop, costs tab                                    |
| [Nimbalyst](https://github.com/Nimbalyst/nimbalyst)       | MIT               | 1.9k  | Free / $20 Teams  | Successor to Crystal, iOS with voice and diff approval       |
| [herdr](https://github.com/herdrdev/herdr)                | Apache-2.0        | 47.1k | Free              | Rust TUI multiplexer over real CLI PTYs, agent state via hooks/output, detach/reattach, `--remote`, socket API for agents to spawn siblings |
| dmux, CCManager                                           | MIT               | ≤1.8k | Free              | TUIs                                                         |
| [Parallel Code](https://github.com/johannesjo/parallel-code) | MIT            | 1.0k  | Free              | QR phone pairing, Docker sandbox per project                 |
| [Sculptor](https://github.com/imbue-ai/sculptor)          | MIT               | 238   | Free              | Research preview, Docker backend                             |
| [Conductor](https://conductor.build)                      | ❌                | –     | Free / $50 Pro    | macOS + iOS, stacked PRs, cloud microVMs                     |
| [Superconductor](https://superconductor.com)              | ❌                | –     | $128/mo           | Cloud VMs, $ per run, 5h/7d usage, team takeover             |
| Agentastic, Polyscope, Vibe Island                        | ❌                | –     | Free / paid       | macOS apps: manager agent, relay, notch monitor              |
| JetBrains Air, Zed                                        | ❌ / GPL          | –     | –                 | Editors running several agents over ACP                      |

## Hosted background-agent platforms

Agents run in vendor sandboxes; you manage tasks from a web or mobile UI.

| Product                         | Agents                          | Pricing                     | Notes                                                        |
| ------------------------------- | ------------------------------- | --------------------------- | ------------------------------------------------------------ |
| [Tembo](https://tembo.io)       | CC Cx Cursor Gm oc Amp Pi       | $60 / $200 + compute        | Slack/Linear/Sentry triggers, schedules, multi-repo          |
| Replicas                        | CC Cx Cursor oc Pi              | $50 / $200 per seat         | Auto-reacts to CI failures and review comments               |
| Niteshift                       | CC Cx Cursor oc Pi              | Free / $50 / $250           | Web and mobile start                                         |
| Boxes.dev                       | CC Cx                           | $19 / $99                   | iOS/Android with terminal                                    |
| Ellipsis                        | CC Cx                           | Free for individuals        | YAML-defined agents                                          |
| Devin                           | own                             | $20 / $200 / Teams          | Mid-run chat and voice, Devin manages Devins                 |
| Factory Droids                  | own                             | $20 / $100 / $200           | Relay to your machines, mobile approvals, session takeover   |
| Amp                             | own                             | free with BYO compute       | Shared runners, agent-to-agent messaging, iOS                |
| OpenHands                       | own                             | MIT core, Enterprise        | Self-host and multi-user need Enterprise                     |
| Coder Agents                    | own                             | self-hosted                 | Replaced Coder Tasks                                         |
| Kilo, Cline, Charlie, Augment, Kiro, Vorflux, Traycer, Nori | own | various           | Single-agent cloud runners                                   |

## Review, tracking and boards

| Product                                        | License          | Notes                                                                     |
| ---------------------------------------------- | ---------------- | ------------------------------------------------------------------------- |
| [Blume](https://blume.codes)                   | ?                | "Deliveries": brief, task tree, runs, revision, tests, screenshots, one approval. Waitlist; earlier 2026 it was a corrections-to-rules app, so it pivoted |
| Linear agents                                  | ❌               | Agents assigned issues in Linear                                          |
| [Backlog.md](https://github.com/MrLesk/Backlog.md) | MIT          | Markdown task board for agents                                            |
| Taskmaster                                     | MIT + Commons Clause | Task decomposition via CLI/MCP                                        |
| Beads / Gas Town                               | MIT              | Agent issue tracker and tmux orchestrator                                 |
| Kanbots, Kanban Code, AI Agent Board, Claudine | OSS              | Vibe Kanban clones                                                        |
| CodeRabbit, Graphite                           | ❌               | AI PR review                                                              |

## Transcripts, usage and observability

Read-only: none can send input or answer prompts.

| Product                                         | License | Notes                                                    |
| ----------------------------------------------- | ------- | -------------------------------------------------------- |
| [ccusage](https://github.com/ryoppippi/ccusage) | MIT     | 19.0k stars, token/cost reports for 18+ agents           |
| Claude-Code-Usage-Monitor                       | MIT     | 8.7k stars, live limit monitor                           |
| claude-devtools                                 | MIT     | 4.0k stars, inspects other machines over SSH             |
| claude-code-log                                 | MIT     | Transcript viewer, full-text search, watch mode          |
| SpecStory                                       | ~       | Captures sessions across tools, cloud sync               |
| Agentlytics                                     | ISC     | Analytics across 17 editors                              |
| Sidekick Agent Hub                              | OSS     | Auto-switches account at a quota threshold               |
| Langfuse, Helicone                              | OSS     | Claude Code plugin / proxy tracing                       |
| Claude Code OTel export + SigNoz/Grafana/Datadog | –      | Vendor telemetry into dashboards                         |

## Defunct or absorbed

Terragon (shut down 2026-02, code released Apache-2.0), Vibe Kanban's company Bloop
(2026-04), Crystal (deprecated for Nimbalyst), Catnip (repo gone), Codegen (ClickUp),
Continue Mission Control (Cursor), Ona / Gitpod (OpenAI), Kilo (Anaconda), Coder Tasks
(retired), Sweep (JetBrains plugin only), BeachViber (dead), ccremote (winding down in
favour of Remote Control).
