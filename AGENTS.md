# AGENTS.md

Guidance for agents and contributors working in this repository.

See [DESIGN.md](./DESIGN.md) for the webui design conventions (Svelte 5,
Tsumikit, atomic design).

## Repo layout

- `crates/cctui-server` — the server (HTTP API + WebSocket).
- `crates/cctui-daemon` — long-lived per-machine daemon that spawns/observes sessions.
- `crates/cctui-admin`, `crates/cctui-proto` — share the workspace version.
- `crates/cctui-tui` — the terminal client (`cctui`). Maintained; see
  [The TUI is a first-class client](#the-tui-is-a-first-class-client).
- `webui/` — the web UI (Svelte 5 + Tsumikit). See DESIGN.md.
- `migrations/` — sqlx Postgres migrations, applied on server start. See
  [docs/database-performance.md](docs/database-performance.md) for reading
  `pg_stat_statements` and index usage before adding or dropping an index.

## The TUI is a first-class client

`crates/cctui-tui` is a supported client, not a leftover. It talks to the same
HTTP API and WebSocket as the webui, and it is the only client available over
plain SSH. Treat a TUI regression the way you would a webui regression.

### Crate layout

- `src/app/` — the store. `state.rs` holds `App`; `action.rs` defines the
  `Action` vocabulary every input source funnels into and the `Effect` set that
  is the only route to the network; `reduce.rs` is
  `reduce(&mut App, Action) -> Vec<Effect>` and is **pure** — no clock (read
  `App::clock_ms`), no IO, no `await`; `effects.rs` runs effects sequentially on
  a worker task, off the key-handling path; `router.rs` is the view stack;
  `toast.rs` is the status-line primitive; `server_event.rs` turns a
  `ServerEvent` into actions; `line.rs` turns an `AgentEvent` into a rendered
  conversation line.
- `src/keys.rs` — pure terminal-input-to-`Action` mapping, testable without a
  terminal. Key handlers dispatch actions; they never call the network.
- `src/views/` — one module per view, plus `views::render` which dispatches on
  the router. `src/widgets/`, `src/theme.rs` — shared chrome.
- `src/ui/` — vendored Codex render modules (markdown, diffs, wrapping), held
  outside the pedantic lints.
- `src/main.rs` — CLI, terminal setup and the select loop. New state belongs in
  the store, not here.

### Server and proto changes must reach the TUI

`app/server_event.rs` matches `ServerEvent` **exhaustively**, so a new variant
will not compile until the TUI does something with it. There are exactly two
acceptable answers:

1. **Handle it** — dispatch an action, however small (a toast counts).
2. **Waive it** — give the variant its own match arm with a one-line reason on
   the arm, e.g. `waived("the TUI has no drafts view")`. The waiver list is
   meant to be read in review; do not add a variant to someone else's arm to
   make it compile.

The same applies to `cctui-proto` wire shapes: `src/server_event_contract.rs`
constructs one sample per `ServerEvent` variant and decodes it through the
TUI's own `client::decode_frame`. Adding a variant breaks that file's
compilation until it has a sample. Keep `VARIANT_COUNT` in step.

Nothing the TUI receives may be dropped silently. Frames that fail to decode
become `Incoming::Undecodable`, and unreadable agent events are counted; both
log and bump a status-line counter.

### The parity manifest

`crates/cctui-tui/parity.toml` has one entry per route in
`cctui_proto::api::routes::ROUTES` and per `ServerEvent` variant, each marked
`tui = "handled"` (naming the module), `"planned"` (naming the epic ticket that
will handle it) or `"waived"` (with a reason). `src/parity.rs` fails
`cargo test` when a route or variant is missing, when an entry names a route
that no longer exists, when a handled route is not mentioned by the module it
names, or when a handled `ws_event` disagrees with the arms in
`app/server_event.rs`.

Adding a route to the table therefore means adding an entry here, and shipping
a handler means flipping its `planned` entry to `handled` in the same change.

### Testing

Views are covered by `insta` snapshots rendered into a ratatui `TestBackend`
(`src/testsupport.rs`, `src/view_snapshots.rs`). Fixtures must not depend on
the wall clock — anything derived from `now` makes a snapshot drift. Regenerate
with `cargo insta accept` and review the diff; never hand-edit a `.snap`.
Reducer and keymap changes belong in the unit tests next to them.

## Package manager: webui is npm

`webui` is a SvelteKit app whose toolchain (adapter-static, paraglide,
Playwright) is exercised on npm and is pinned by `webui/package-lock.json`. Its
`package.json` declares `packageManager` and a `preinstall` guard that refuses
another tool with a readable message: a foreign lockfile silently resolves a
different dependency tree. `make webui/install` is the supported entry point.

## Versioning

**Bump the version alongside the change that needs it.** Don't leave version
bumps for a separate follow-up — they belong in the same change as the code.

- **Rust crates** share a single workspace version in the root `Cargo.toml`
  (`[workspace.package].version`); all crates inherit it. After bumping, run
  `cargo update -p cctui-server -p cctui-daemon -p cctui-tui -p cctui-admin -p cctui-proto --precise <ver>`
  so `Cargo.lock` matches.
- **webui** has its own `webui/package.json` `version`; bump it when the UI changes.
- Bump the semver in the appropriate manifest for whatever you touched. Use
  semver intent: patch for fixes, minor for features, major for breaking changes.

## Pull requests

- Code changes go through a **branch → PR**, and the **version bump lives in the
  same PR** as the change — not a separate one.
- Keep PRs focused; reference the relevant ticket in the title where applicable.
- Let the pre-commit hooks (lefthook) run fmt / check / clippy / lint.

## Self-update

The webui's "Update" button (`POST /api/v1/version/self-update`) has two paths,
and the deterministic one is the default whenever it is available.

**Prefer the update hook.** A daemon with `CCTUI_UPDATE_COMMAND` set advertises
it on every heartbeat; the server then hands that machine the target version and
it runs the operator's own command, verifies the served version, and rolls back
on failure. No model, no account, the same bytes every release. The contract and
per-platform recipes live in [docs/update-hook.md](./docs/update-hook.md); the
code is `crates/cctui-daemon/src/updatehook.rs` and
`crates/cctui-server/src/routes/update_hook.rs`.

Don't teach the server how any deployment updates. There are as many answers as
there are installations, and the operator already knows theirs.

### Agent fallback: model floor

With no hook on the target machine, the button spawns a YOLO agent there
instead. That agent reads a deployment's runbook and acts on infrastructure, so
it must **never run on a small model**:

- Claude: always a tier **above Sonnet** (Opus or better), `medium` effort.
- OpenAI: a GPT-5-class frontier model, `medium` effort; never a `mini` / `nano`
  variant.

The floor lives in `launch_profile()` in
`crates/cctui-server/src/routes/self_update.rs`. **Raise it whenever a newer
generation replaces those names** — treat it as part of any model-catalog bump,
and never lower it to save cost.

## Releases

After a PR merges, **cut a release so the package is built/published**:

- **Check TUI parity before tagging.** `cargo test -p cctui-tui` must be green,
  and `git diff <last tag>.. -- crates/cctui-tui/parity.toml` must be read, not
  skimmed: every `planned` entry whose ticket shipped in this release has to be
  `handled` by now. The manifest test is the gate — a route or `ServerEvent`
  variant that reached the API without an entry fails it, and so does a
  `handled` entry with no handler behind it. Fix the manifest, don't waive to
  get green.
- Tag the new version on the default branch; CI builds and publishes the
  artifacts/images for that tag.
- Pick the channel with the tag: `vX.Y.Z` is stable, `vX.Y.Z-beta.N` is a beta
  pre-release. See [docs/release-channels.md](./docs/release-channels.md).
- The webui ships as its own image/overlay independent of the server.
- Verify the release actually rolled out before considering the work done —
  don't stop at "PR merged" or "tag pushed".
