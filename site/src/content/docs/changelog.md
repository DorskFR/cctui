---
title: Changelog highlights
description: The milestones that shaped cctui, from the first public release to today.
---

cctui ships several times a day on a stable and a beta channel. These are the milestones that changed what it is; the full notes are on [GitHub Releases](https://github.com/DorskFR/cctui/releases).

| When | Version | Milestone |
|---|---|---|
| July 2026 | v0.7.134 to v0.7.141 | Full Codex support: protocol handshake, thread discovery, steering, codex-native dispatch and context packs |
| July 2026 | v0.7.154 | Live PTY terminal view of any session; agents can post images |
| July 2026 | v0.7.190 to v0.7.195 | Guard-proxy sidecar: TLS-terminating credential injection, so worker pods never see real secrets |
| July 2026 | v0.7.196 to v0.7.199 | Kubernetes `WorkerProfile` CRD with injection and validating admission webhooks, plus the orchestrator image |
| July 2026 | v0.7.226 to v0.7.227 | opencode adapter, dollar soft limits, and the `CctuiAgent` tool: agents spawn other agents |
| August 2026 | v0.7.254 | `CctuiAgent` becomes a real subagent tool: parallel calls, live progress, turn completion, follow-ups |
| September 2026 | v0.7.305, v0.7.312 | Passkey sign-in; account pools that bound auto-binding and failover |
| September 2026 | v0.19.0 | Scheduled messages, follow-up sessions, prompt-cache keep-alive, usage history |
| September 2026 | v0.20.0 | Hardening release: multi-user isolation, minisign-signed self-update, stable and beta channels |
| September 2026 | v0.22.1 | Runtime plugins, the yubisashi visual-feedback mod, app previews proxied through cctui |
| September 2026 | v0.23.0-beta.10 | Peer messaging and rooms between agents, the tiles view, the Needs-you list |
| October 2026 | v0.23.0 | Full terminal client at day-to-day parity with the web UI, with device-code login |
| October 2026 | v0.25.0-beta.2 to beta.5 | ACP adapter (any Agent Client Protocol agent), voice notes from agents, speech service |
| October 2026 | v0.25.0-beta.9 | cctuiverse: signed session-to-session links across different cctui servers and owners |
| October 2026 | v0.25.7 to v0.26.0 | Live two-way voice with barge-in, per-user dispatch policies, failed-dispatch cards with re-run |
| October 2026 | v0.26.1 betas | Streamed spoken replies, per-profile guard-proxy settings |

## Things that were removed on purpose

- The built-in GitHub review centre became an external mod installed from the catalog.
- Implicit cross-account balancing was removed; failover is explicit or pool-bound.
- Default subagent limits (16 children, depth 3, $400 per tree) were dropped; set them explicitly if you want them.
- Dispatcher and daemon `?token=` query auth was removed in favour of header-only auth.
- The TUI's admin, account-editing and dispatch screens were cut back; the web UI holds those.
- Memory auto-attach became suggestions only; rules still auto-attach.
