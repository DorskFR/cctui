# ADR 0002 — Session context: skills, memory and prompt templates

- **Status:** Accepted
- **Date:** 2026-09-30
- **Deciders:** cctui maintainers
- **Closes the decision asked for by:** the skills-distribution and
  memory-design tickets
- **Builds on:** the plugin platform (`docs/plugins.md`), the neutral launch
  preamble and preflight (`crates/cctui-daemon/src/{preamble,preflight}.rs`),
  the per-session gateway-env pull, the bootstrap upload staging seam
  (`adapters/uploads.rs`), and the harness parity map
  (`docs/harness-parity.md`)

## Context

Three long-standing tickets ask variations of one question: can a user keep
reusable context in cctui — skills, durable notes, prompt templates — and have
it attached to a session at spawn, on a local daemon as well as a dispatched
worker, for every harness?

They were written against a tree that no longer exists. What is true now:

- **Skills are solved, by the plugin platform.** `plugins::resolve_session_skills`
  mirrors a plugin's `skills/` per session and delivers it to each harness
  through the channel that harness has: `plugin_dirs` for claude-code,
  a `<cctui_skills>` catalog in `developerInstructions` for codex,
  `skills.paths` for opencode. It is per-user (enabled flags in user settings),
  versioned (`version` + `skills_hash`), cached, and it reaches local daemons
  and dispatched workers alike because it rides the gateway-env pull.
- **The old skill registry never got a consumer.** `skill_store.rs`,
  `routes/skills.rs` and table `skill_registry` still exist; the only producer
  is `cctui-admin skills push`, and nothing downloads a bundle. Its name is a
  global primary key, and it writes a sha256 into the `version` column.
- **Memory does not exist.** Nothing injects durable per-user or per-project
  text into a local session.
- **Prompt templates half-exist.** The `prompts` table and `routes/prompts.rs`
  serve GitHub "Review with agent"; `SpawnRequest.prompt_name` is dead — every
  producer sends `null` and nothing reads it.
- **There is now a neutral injection seam.** `preamble.rs` builds one
  harness-neutral block and each adapter implements a one-line delivery
  primitive. Today it carries only the shared-checkout notice.
- **There is one launch-time server→daemon channel.** `GatewayEnvResponse`
  already carries `env`, `settings`, `whip_phrases`, `spawn_capability` and
  `plugins`, and all three adapters pull it through
  `gateway_env::resolve_launch` at every spawn, fork, resume and cold-resume.

## Decision drivers

1. **One distribution model, not two.** The tickets predate the plugin
   platform and propose building a second skill-delivery path beside it.
2. **One injection seam, not three.** The parity map's standing complaint is
   that capabilities get written per harness; anything added here must not
   repeat that.
3. **Local daemons and dispatched workers must behave identically.** Context
   packs fail this: they are pod-level and invisible to a local session.
4. **The webui and `CctuiAgent` must produce the same kit.** Profiles are
   applied client-side today, so a child session silently gets a different
   setup from a webui spawn of the same profile.
5. **Committed unverified.** This lane runs nothing, so the change must reuse
   proven seams rather than introduce new runtime machinery.

## Decision

### 1. Skills are plugins. The old registry is deprecated, not extended.

A skill bundle in cctui is a plugin that ships `skills/`. We do not build a
second per-user skill store, a second staging path, or a second per-harness
delivery. The skills half of the skills ticket is **already shipped**; what was
missing was never the registry, it was the consumer, and the plugin platform is
that consumer.

`skill_registry` / `skill_store.rs` / `routes/skills.rs` / `cctui-admin skills
push` are frozen: no new features, no session association. Removing them is a
follow-up, not part of this work — they are inert and deleting a table is not
free.

Consequence: "attach skill X to this session" is expressed as "enable plugin X",
which is per-user rather than per-session. Per-session skill *selection* is a
genuine remaining gap and is recorded as a non-goal below.

### 2. Context items are memory notes and prompt templates.

One table, one kind column, because they share every other column and the same
resolution and injection path:

```
context_items(
  id, user_id,
  kind      text  -- 'memory' | 'prompt'
  name      text  -- stable, unique per (user_id, kind)
  title     text
  body      text  -- markdown
  scope     text  -- 'user' | 'machine' | 'path' | 'label'
  scope_ref text  -- machine id | path prefix | label id; NULL for 'user'
  tags      text[]
  enabled   bool
  version   int   -- bumped on every body/scope change
  created_at, updated_at
)
```

- **Namespacing** is `(user_id, kind, name)`. Names are slugs
  (`^[a-z0-9][a-z0-9-]{0,63}$`) so they are stable references from profiles and
  from a spawn request, and safe as filenames when staged.
- **Scopes** resolve as a union, not most-specific-wins — unlike repo-scoped
  prompts, several memories legitimately apply at once:
  - `user` — always,
  - `machine` — when the spawn targets that machine,
  - `path` — when the spawn's `working_dir` is at or under `scope_ref`
    (component-wise prefix, so `/src/foo` does not match `/src/foobar`),
  - `label` — when the spawn carries that label id.
- **Versioning** is a monotonic `version` int, bumped on edit. No history
  table: a note is a living document, and the audit trail nobody asked for is
  the kind of thing that never gets read. The version exists so a card can say
  which revision a session received and so a future cache has a key.
- **Prompts fold in over time, they are not migrated now.** The existing
  `prompts` table keeps serving review prompts with its repo scoping. A prompt
  *template* here is a different thing: a first-turn body with variables. When
  the review flow is next touched, it can move onto `context_items` with
  `scope='label'`; forcing that migration now would break a working feature for
  tidiness.

### 3. Resolution happens once, server-side, at spawn.

`SpawnRequest` gains:

```
context: { items: [name], auto: bool }   // default { [], auto: false }
```

The server resolves the effective set = explicit picks ∪ (auto ? scope-matching
enabled items : ∅), and **persists it keyed by the spawn's launch key**, the
same key `spawn_capabilities` and the label/follow-up intents already use.

**`auto` ships OFF.** Scope resolution is built, tested and reachable
(`GET /context/resolve`), but nothing is attached to a session unless someone
named it. Injecting text into every session on a machine because a directory
matched is not a default to turn on before the spawn panel can show what a
scope would pull in and before the feature has run in anger. The switch is one
boolean; the ordering is deliberate.

This is the load-bearing choice. Because resolution is server-side and keyed by
the launch key:

- a webui spawn, a `CctuiAgent` child and a dispatched worker go through the
  same code path and get the same kit;
- **profiles gain a `context` set applied server-side**, fixing the standing
  bug that profiles are applied by overwriting the spawn form in the browser
  and are therefore invisible to children and to dispatch;
- nothing has to be threaded through `SessionSpec`, which every adapter would
  then have to read separately.

### 4. Delivery uses the two seams that already exist, and adds none.

The daemon receives the resolved items on the **gateway-env pull** — the one
launch-time channel all three adapters already make (`GatewayEnvResponse.context`,
beside `plugins`). Then, per session:

- **Bodies are staged as a file**, `context.md`, through the existing
  `adapters::uploads::stage_files` seam (0600, per-session dir). One file, with
  each item as a `## <title>` section, rather than one file per item: the agent
  reads one path and the prompt stays short.
- **The agent is told about it through the neutral preamble.**
  `preamble::block` gains a context section naming the staged path and listing
  the item titles. Claude folds it into `<session-context>`, codex into
  `developerInstructions`, opencode into the spawn prompt — the primitives are
  already implemented and unchanged.

We deliberately do **not** use claude's `--append-system-prompt-file` or the
`claudeMd` setting (which the settings catalog marks `managed`). A
claude-only path would reintroduce exactly the per-harness divergence the
preamble was built to remove, and the staged-file-plus-pointer approach is the
one already proven for bootstrap uploads on all three harnesses.

**Prompt templates are not context, they are the prompt.** A selected prompt
template is expanded server-side into the spawn's first turn, with `{{cwd}}`,
`{{name}}` and `{{topic}}` substituted, and the dead `SpawnRequest.prompt_name`
is retired in favour of a `prompt` kind item. Putting a template in the staged
file would make the agent read its own instructions out of a file for no
reason.

### 5. Mid-session attach re-uses the same staging.

Attaching an item to a live session stages the file with `stage_files` and
sends a short user turn naming the path. No restart, no re-launch, no new
daemon command: `StageFiles` already exists and already works mid-chat for
uploads on claude and codex.

### 6. Relation to context packs.

Context packs stay, scoped down to what they are actually good at:
platform-level pod content (hooks, guard rules, MCP wiring, the base image's
own files). Per-session content — memory, prompt templates, skills — moves to
the mechanisms above, which work identically for a local daemon session and a
worker pod. A pack's `skills/` becomes redundant once the plugin is installed
server-side; we do not remove it here.

## Non-goals

- **Per-session skill selection.** Plugins are enabled per user. Choosing a
  subset of skills for one session is a real gap; it belongs to the plugin
  platform, not to a competing store.
- **Auto-extracting memories from transcripts.** A "remember this" action from
  selected transcript text is a natural follow-up; inferring memories
  automatically is not on the table.
- **Migrating review prompts** off the `prompts` table.
- **Deleting `skill_registry`.**
- **Moving the rest of `build_session_context`** (name, model·effort,
  permission posture, env names, attachments, the `CctuiAgent` paragraph) onto
  the neutral block. This ADR makes that cheap — the block gains a real section
  structure — and the parity map ranks it as gap #2, but it is a separate
  change with its own blast radius.

## Consequences

**Good.** One skill model instead of two. Memory and prompt templates work on
local daemons, which context packs never did. Profiles finally mean the same
thing from the webui, from `CctuiAgent` and from dispatch, because the server
resolves them. No new per-harness code: the preamble and the staging seam each
gain one caller.

**Bad.** Until the spawn panel grows its picker, attaching a memory means
typing its name, so the scope machinery is dormant in practice. `context_items`
overlaps conceptually with `prompts` until the review flow moves, so there are
two prompt stores for a while. The staged file costs
the agent a read it would not need if the text were in a system prompt. Scope
resolution is a union, so a user with many broad-scoped memories can quietly
inflate every session's context — the editor must show what a given cwd would
resolve to.

**Risky.** Memory bodies are user-authored text that reaches any session they
are attached to — and, once `auto` is switched on, every session whose scope
matches; they ride the gateway-env pull, which is already the channel for
credentials, so the existing user/machine ownership check governs them. Items
are never logged.
