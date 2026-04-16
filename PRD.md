---
Product: cctui (Claude Code Control TUI)
Version: 0.1
Date: 2026-03-30
Status: MVP Shipped
---

# Product Requirements Document: cctui

---

## 1. Vision & Problem Statement

### Vision
Enable centralized management and visibility of Claude Code sessions across multiple machines, with real-time conversation monitoring, policy enforcement, and multi-user collaboration.

### Problem Statement
Claude Code agents run independently on individual machines with no central visibility or control. Teams cannot:
- See what agents are doing across machines without SSH'ing into each one
- Enforce consistent policies across agent sessions
- Share credentials, prompts, or skills across machines
- Track token usage or costs across sessions
- Coordinate work between multiple agents

### Target Users
- **Primary**: Teams running Claude Code agents (2-50 people)
- **Secondary**: Individual power users managing multiple machines
- **Tertiary**: Organizations with compliance/policy requirements

---

## 2. Core Product

### 2.1 Session Registry & Visibility
**What**: Centralized server that registers and tracks all Claude Code sessions
**How**:
- Claude Code agent registers itself on startup via `SessionStart` hook
- Registration includes: machine name, session ID, transcript path, working directory, start time
- Sessions are listed in TUI grouped by machine
- Sessions marked as "live" while connected; marked "terminated" if disconnected >5min or idle >90s

**Success Criteria**:
- Agent can register within 1 second of startup
- TUI can list 100+ historical sessions with sub-second latency
- Session status updates appear in TUI within 2 seconds

### 2.2 Conversation Streaming & Persistence
**What**: Real-time capture of conversation history (user messages, assistant responses, tool calls)
**How**:
- Channel MCP server spawned by Claude Code tails transcript JSONL file
- Events streamed to server via HTTP POST
- Server persists events to PostgreSQL
- TUI fetches conversation history from server on session selection
- Live events appear in TUI via WebSocket subscription

**Success Criteria**:
- Conversation appears in TUI within 2 seconds of generation
- Full conversation history available even if session disconnected
- Can handle 50+ tool calls per session without data loss

### 2.3 TUI: Live Session Monitoring
**What**: Terminal UI for viewing sessions and conversations in real time
**How**:
- Session list view: all machines, filterable by status/machine name
- Conversation view: chronological view with timestamps, role-based colors
- Live scroll to latest message
- Message formatting: preserve markdown, syntax-highlighted code blocks, clean XML/ANSI escape stripping
- Keybindings: j/k for navigation, i for input, ? for help, g/G for jump to top/bottom

**Success Criteria**:
- TUI renders 500+ messages without lag
- Terminal resize doesn't break layout
- Markdown renders cleanly (bold, code blocks, lists, links)
- Tool calls display concisely (command visible, inputs collapsible)

### 2.4 Policy Enforcement
**What**: Central policy engine that controls what tools agents can use
**How**:
- `PreToolUse` hook proxies tool calls to server for approval
- Server evaluates tool against per-session policy rules
- Rules can be markdown-based (allowed/disallowed/transition blocks)
- Current implementation: allow-all (placeholder)

**Success Criteria**:
- Policy check latency <100ms
- Policies can be per-session or per-workflow
- Policies can reference session metadata (machine, user, branch, etc.)
- Failed checks block the tool and log the denial

### 2.5 Bidirectional Messaging
**What**: TUI can send commands/messages to running Claude agents
**How**:
- TUI sends command via server → channel notification → agent receives via MCP notification
- Agent replies via `cctui_reply` tool → posted to server → broadcast to TUI
- Example: TUI can request agent to pause, save, or context-switch

**Success Criteria**:
- Message delivery <500ms latency
- Agent can reply or ignore messages without breaking flow
- Multiple TUI clients can message same agent without conflicts

---

## 3. Technical Architecture (Summary)

```
┌─────────────────┐
│  Claude Code    │
│  (on machine)   │
└────────┬────────┘
         │ SessionStart, PreToolUse, transcript tailing
         ↓
┌─────────────────────────────────────────┐
│  cctui-channel (Bun/TypeScript MCP)     │
│  • HTTP hook server                     │
│  • JSONL transcript tailer              │
│  • REST client to cctui-server          │
│  • MCP notification listener            │
└────────┬────────────────────────────────┘
         │ HTTP POST events + policy checks
         ↓
┌──────────────────────────────────────┐
│  cctui-server (Rust/Axum)            │
│  • Session registry (PostgreSQL)      │
│  • Policy engine                      │
│  • WebSocket hub (live fan-out)       │
│  • Event storage                      │
└────────┬─────────────────────────────┘
         │ WebSocket events + REST API
         ↓
┌──────────────────────────────────┐
│  cctui-tui (Rust/Ratatui)        │
│  • Session list view             │
│  • Conversation viewer           │
│  • Chat input                    │
│  • REST + WebSocket client       │
└──────────────────────────────────┘
```

**Key Components**:
- **cctui-proto**: Shared types (Session, SessionStatus, AgentEvent, TuiCommand)
- **cctui-server**: HTTP routes, WebSocket handler, PostgreSQL integration, session reaper
- **cctui-tui**: Ratatui UI, REST/WebSocket client, conversation renderer
- **channel**: MCP server bridge, HTTP hook receiver, transcript streamer

---

## 4. Use Cases

### Use Case 1: Monitor a Running Agent
**Actor**: Team lead
**Flow**:
1. Open TUI, see list of agents by machine
2. Select an agent
3. Watch conversation in real time (user input, assistant work, tool calls)
4. See tool calls get approved/denied by policy engine
5. TUI updates within 2 seconds

**Success Criteria**: Team lead can monitor agent without SSH

### Use Case 2: Enforce a Workflow Rule
**Actor**: Operations / Compliance
**Flow**:
1. Define policy: "Agents on `main` branch cannot use `Bash` tool"
2. Upload to server
3. Agent tries to use Bash on main → server denies → TUI shows "Policy blocked: Bash"
4. Agent can retry on dev branch

**Success Criteria**: Policy prevents tool use without manual intervention

### Use Case 3: Share Prompts & Skills Across Machines
**Actor**: Team lead
**Flow**:
1. Upload shared CLAUDE.md to server
2. On agent startup, channel pulls latest CLAUDE.md and injects it
3. All agents have same prompt library without manual sync

**Success Criteria**: Agents start with current prompts without extra setup

### Use Case 4: View Historical Sessions
**Actor**: Analyst
**Flow**:
1. TUI shows sessions from past week (filtered by machine/status)
2. Click on terminated session
3. View full conversation history from DB
4. Search/filter by keyword or tool type

**Success Criteria**: Can audit past sessions without server logs

---

## 5. Feature Roadmap (Prioritized)

### Phase 1: MVP (Current — v0.1)
✅ Session registration & listing
✅ Conversation streaming & persistence
✅ TUI: basic session/conversation views
✅ Channel MCP server & hooks
✅ Placeholder policy engine (allow-all)

### Phase 2: TUI Polish (High Priority)
- [ ] Markdown rendering (bold, lists, code blocks)
- [ ] Clean message display (strip XML tags, ANSI escapes)
- [ ] Borderless layout (spacing + color instead of boxes)
- [ ] Syntax highlighting for code blocks
- [ ] Active session tabs/sidebar for quick switching
- [ ] Tool call formatting (collapse long inputs)
- [ ] Page up/down, mouse scroll support
- [ ] Terminal resize handling

### Phase 3: Policy Engine (Medium Priority)
- [ ] Markdown-based rule syntax
- [ ] Per-session policy assignment
- [ ] Policy templates (e.g., "dev vs. production")
- [ ] Audit log of policy decisions
- [ ] Manual override capability

### Phase 4: Credentials & Account Management (Medium Priority)
- [ ] Vault backend for API keys (K8s integration)
- [ ] Account picker on session registration
- [ ] Support multiple Claude accounts per team
- [ ] Token scoping (which agent can access which credentials)

### Phase 5: Prompt & Skill Library (Medium Priority)
- [ ] Central repository of CLAUDE.md files
- [ ] Version management
- [ ] Push to agents on registration
- [ ] Template system for common workflows

### Phase 6: Dashboard & Analytics (Low Priority)
- [ ] Token usage tracking (per-session, per-machine, per-day)
- [ ] Cost aggregation
- [ ] Tool usage heatmap
- [ ] Performance metrics (latency, error rates)

### Phase 7: Multi-Machine Production Deployment (Low Priority)
- [ ] TLS/HTTPS everywhere
- [ ] Agent binary distribution from server
- [ ] Lima VM / remote machine support
- [ ] Comprehensive deployment guide
- [ ] Load testing for 100+ concurrent sessions

---

## 6. Success Metrics

| Metric | Target | Owner | Measurement |
|--------|--------|-------|-------------|
| Session registration latency | <1s | Server | HTTP response time |
| Message delivery latency | <2s | Channel | Timestamp diff |
| Policy check latency | <100ms | Server | HTTP response time |
| TUI render latency | <500ms | TUI | Frame time |
| Session persistence | 99.9% | Server | Event loss rate |
| Policy enforcement | 100% | Server | Denied tool calls match rules |
| User adoption | 5+ teams | Product | Internal surveys |

---

## 7. Out of Scope (v1)

- Audio/video streaming of agent work
- AI-powered insights (anomaly detection, recommendations)
- Integration with external issue trackers (Jira, GitHub)
- Email alerts (can be built on top of webhooks)
- Mobile app
- On-prem air-gapped deployment (future consideration)

---

## 8. Dependencies & Risks

### Dependencies
- Claude Code stability (SessionStart/PreToolUse hooks)
- PostgreSQL availability (session/event storage)
- Network reliability between agent and server

### Risks
- **Hook reliability**: If SessionStart hook fails, session won't register → visibility lost
  - Mitigation: Log hook errors; add hook health check endpoint
- **Transcript file timing**: File doesn't exist when SessionStart fires → streamer must retry
  - Mitigation: Channel already implements exponential backoff
- **Scale**: 500+ concurrent sessions may overwhelm WebSocket hub
  - Mitigation: Implement fan-out sharding, room-based subscriptions
- **Policy latency**: Central check could become bottleneck
  - Mitigation: Client-side caching, policy versioning for quick lookups

---

## 9. Design Principles

1. **Minimal agent footprint** — No heavy dependencies on Claude side; use existing hooks
2. **Real-time visibility** — Events in TUI within 2 seconds
3. **Graceful degradation** — If server is down, agents continue working; TUI shows stale data
4. **Auditability** — All tool calls and policy decisions logged
5. **Flexibility** — Policy rules, prompts, and workflows are user-configurable
6. **Developer experience** — Simple `make setup` for onboarding

---

## 10. Glossary

| Term | Definition |
|------|-----------|
| **Session** | A single Claude Code agent execution, identified by `session_id` |
| **Transcript** | JSONL file in `~/.claude/transcript` containing conversation events |
| **Hook** | Script triggered by Claude Code (SessionStart, PreToolUse, Stop) |
| **Policy** | Rule that allows/denies tool use based on session metadata |
| **Channel** | MCP server spawned by Claude Code; bridges agent ↔ cctui-server |
| **TUI** | Terminal UI for monitoring sessions and conversations |
| **Reaper** | Background task that marks idle/disconnected sessions as terminated |

---

## 11. Success Criteria for v0.1 → v1

- [ ] 3+ teams using cctui in production
- [ ] Zero data loss incidents (transcript completeness = 100%)
- [ ] TUI handles 100+ concurrent sessions without performance degradation
- [ ] Policy engine blocks at least one category of risky tool use
- [ ] Documentation covers installation, setup, and basic workflow
- [ ] All core features have unit + integration tests (>80% coverage)
