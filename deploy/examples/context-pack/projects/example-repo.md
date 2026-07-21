# Per-repo overlay (context pack example)

A `projects/` entry is a per-repo `CLAUDE.md` overlay: repo-scoped instructions
layered on top of the home-level `CLAUDE.md` for tasks that touch a specific
repository. The dir is copied to `/home/worker/projects/` at boot.

This fixture is NEUTRAL — name the file after the repo it targets and put that
repo's build/test commands, layout notes, and gotchas here. Replace it with your
own overlays (or drop the dir) when you build a derived pack.

## example-repo

- Build: `make build`
- Test: `make test`
- Layout: `src/` for code, `tests/` for tests (placeholders).
