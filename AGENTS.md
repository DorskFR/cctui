# AGENTS.md

Guidance for agents and contributors working in this repository.

See [DESIGN.md](./DESIGN.md) for the webui design conventions (Svelte 5,
Tsumikit, atomic design).

## Repo layout

- `crates/cctui-server` — the server (HTTP API + WebSocket).
- `crates/cctui-daemon` — long-lived per-machine daemon that spawns/observes sessions.
- `crates/cctui-admin`, `crates/cctui-proto` — share the workspace version.
- `crates/cctui-tui` — is unmaintained for now.
- `webui/` — the web UI (Svelte 5 + Tsumikit). See DESIGN.md.
- `migrations/` — sqlx Postgres migrations, applied on server start.

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

## Releases

After a PR merges, **cut a release so the package is built/published**:

- Tag the new version on the default branch; CI builds and publishes the
  artifacts/images for that tag.
- The webui ships as its own image/overlay independent of the server.
- Verify the release actually rolled out before considering the work done —
  don't stop at "PR merged" or "tag pushed".
