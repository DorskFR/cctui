# CI and release pipeline benchmarks

Numbers come from GitHub Actions history, not local runs: medians over the most
recent successful runs, measured from job and step `startedAt`/`completedAt`.
Hosted runners are noisy, so compare medians and treat differences under ~10 s
as noise.

## Baseline (before the 0.20 pipeline changes)

### `ci.yml` — 20 successful runs, 2026-09-23 → 2026-09-25

Wall clock (created → last update): **median 192 s**, p90 211 s. Jobs run in
parallel, so the wall clock is the slowest job plus queueing.

| Job | Median | Max |
| --- | ---: | ---: |
| cargo test | 186 s | 196 s |
| webui | 170 s | 193 s |
| cargo test (ignored integration) | 132 s | 172 s |
| cargo clippy | 74 s | 83 s |
| audit | 42 s | 72 s |
| ghreview | 36 s | 54 s |
| ghreview-ui | 29 s | 36 s |
| cargo fmt | 16 s | 19 s |
| generated artifacts up to date | 13 s | 16 s |
| biome / actionlint / no-css-global / i18n-messages | 6–9 s each | |

Heaviest steps:

| Job :: step | Median |
| --- | ---: |
| cargo test :: cargo test (workspace) | 134 s |
| webui :: Test | 99 s |
| cargo clippy :: cargo clippy | 54 s |
| cargo test (ignored integration) :: cargo test --ignored | 48 s |
| cargo test (ignored integration) :: Build the server | 40 s |
| webui :: Build | 31 s |
| webui :: Check | 19 s |
| Postgres service containers (3 jobs) | 16–18 s each |
| Swatinem/rust-cache restore | 8–16 s per Rust job |
| Rust toolchain install | 8–9 s per Rust job |

### `release.yml` — 8 successful runs, 2026-09-22 → 2026-09-25

Wall clock: **median 1193 s** (≈ 20 min), p90 1234 s.

| Job | Median | Max |
| --- | ---: | ---: |
| images | 1008 s | 1058 s |
| build linux-amd64 | 291 s | 589 s |
| build linux-arm64 | 288 s | 364 s |
| build darwin-arm64 | 286 s | 357 s |
| cargo test | 182 s | 193 s |
| release | 12 s | 24 s |

`images` steps: worker 273 s, server 270 s, dispatcher-kube 215 s, orchestrator
144 s, webui 60 s, ghreview 8 s. Both `images` and `build` wait on `test`, so the
critical path is `test` (182 s) followed by `images` (1008 s) ≈ 1190 s, which is
the whole wall clock.

### Other workflows

- `journeys.yml`: median 10 s (10 runs). Not a factor.
- `preview.yml`: one run in the window, 297 s, of which the server image is 226 s.

## Top 3 bottlenecks

1. **Release `images` builds every Rust image from scratch, one after another.**
   1008 s, 85 % of the release wall clock. Each Dockerfile compiles its own copy of
   the workspace (server 270 s, worker 273 s, dispatcher-kube 215 s, orchestrator
   144 s) with no layer cache, while the `build` jobs compile the same binaries in
   parallel with it. It also starts only after a `cargo test` that CI already ran
   on the same commit.
2. **Rust compile and link time dominates `cargo test`.** In run 36123748130 the
   workspace test step spent 98 s in `Finished test profile … in 1m 38s` with a
   warm rust-cache, and about 36 s running tests. The integration job pays twice:
   a 40 s `dev` build of the server and a 49 s `test` build, to run tests that
   finish in under a second each.
3. **webui vitest spends its time importing and building DOMs, not testing.**
   The same run reports `Duration 75.13s (transform 18.44s, import 157.14s,
   tests 15.91s, environment 33.71s)`: 16 s of test execution against 157 s of
   (cumulative, per-worker) module import and 34 s of happy-dom setup, because
   every file ran under happy-dom. On top of that, paraglide and journey codegen
   ran four times per job (`npm ci`'s `prepare`, then `check`, `test`, `build`).

## Candidates

Applied means the change landed in 0.20. "After" numbers are to be filled in from
the first 10+ CI runs and first 3+ releases on 0.20, measured the same way.

| Candidate | Status | Targets |
| --- | --- | --- |
| Path filters (skip Rust jobs on webui-only changes and vice versa) | Applied in 0.20 | Whole-workflow wall clock and runner minutes on single-area PRs |
| Dev/test profile: reduced debuginfo | Applied in 0.20 | Bottleneck 2 (link time, cache size) |
| Integration job targets only the tests it runs | Applied in 0.20 | Bottleneck 2 (the extra 40 s + 49 s builds) |
| vitest runs under `node` by default; DOM tests opt into happy-dom | Applied in 0.20 | Bottleneck 3 (environment, import) |
| Codegen once: `codegen` script from `prepare`, `codegen:ensure` in test/check/build | Applied in 0.20 | Bottleneck 3 (repeat codegen) |
| Docker layer cache for release images | Applied in 0.20 | Bottleneck 1 |
| Build once: images reuse binaries from the `build` jobs | Applied in 0.20 | Bottleneck 1 |
| No duplicate `cargo test` in release (CI already gated the commit) | Applied in 0.20 | Release critical path −182 s |
| `lld`/`mold` linker | Deferred | Worth measuring once debuginfo reduction lands; link time is inside the 98 s compile figure and can't be separated from Actions logs alone. Needs a `cargo build --timings` run. |
| `split-debuginfo` | Deferred | Overlaps with the debuginfo reduction; re-evaluate against its after-numbers. |
| `cargo-nextest` | Rejected for now | Test execution is ~36 s of a 186 s job; nextest parallelism attacks the smaller share. |
| sccache / shared target cache | Deferred | rust-cache already restores in 8–16 s and the compile still takes 98 s. Revisit after path filters and profile changes, when cache hit rate can be read cleanly. |
| CI job splitting | Rejected | CI already runs 13 jobs in parallel; wall clock tracks the single slowest job. |
| Per-test DB schemas to parallelise DB tests | Rejected for now | DB-backed execution is seconds; the Postgres service start (16–18 s) costs more than the tests. |
| Dependency-graph surgery / duplicate crates | Deferred | No evidence yet. Needs `cargo build --timings` and `cargo tree --duplicates`, which are local measurements outside this pass. |

## How to re-measure

Read-only, needs only `gh` authenticated against the repo:

```sh
# Pick the runs
gh run list --workflow ci.yml --status success -L 20 --json databaseId,createdAt,updatedAt
gh run list --workflow release.yml --status success -L 8 --json databaseId,createdAt,updatedAt

# Per-job and per-step durations for one run
gh run view <run-id> --json jobs --jq '
  .jobs[] | select(.conclusion == "success") |
  "\(.name)\t\((.completedAt | fromdate) - (.startedAt | fromdate))s",
  (.steps[] | "  \(.name)\t\((.completedAt | fromdate) - (.startedAt | fromdate))s")'

# vitest and cargo breakdowns for one run
gh run view <run-id> --log | grep -E 'Duration .*transform|Finished `(test|dev)` profile'
```

Take the median of each job and step across the runs. Run wall clock is
`updatedAt - createdAt`. Compare only runs that exercised the same jobs; once
path filters are in, split PR runs by which jobs actually ran.
