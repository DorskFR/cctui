---
name: golden-tests
description: Oracle skill for the `pure-calc` surface — verify deterministic logic (parsing, math, formatting) against golden-file fixtures plus unit/property tests, in-pod, with no I/O. The change is exercised against recorded expected outputs; the passing run IS the review. Run it in the implement step when the classification plan lists `golden-tests`; it produces the `test-run` evidence the evidence-gate demands for `pure-calc`. Deterministic and near-autonomous — a green run unlocks the plan's `auto-merge` path.
---

# golden-tests

The cheapest oracle, for the only surface that earns `auto-merge`. A
`pure-calc` change is a deterministic function of its inputs — no clock, no
network, no filesystem state — so its correctness can be pinned to **golden
files**: recorded input → expected-output pairs that the change is run against.
A passing golden run is not a smoke test, it *is* the review: the oracle is the
reviewer, which is why `pure-calc` is the surface the classifier allows to merge
on green.

Run this in the implement step (Step 3) when the classification plan lists
`golden-tests` among its `oracles[]`. It needs no network and no secrets — the
fixtures are checked into the repo.

## What it verifies

Three layers, cheapest first:

1. **Unit tests** — the change's own assertions over hand-picked cases.
2. **Property tests** — invariants over generated inputs (round-trip,
   idempotence, monotonicity) — catch the cases you did not think to enumerate.
3. **Golden files** — recorded `input → expected-output` fixtures; the change is
   run over each input and its output is diffed against the recorded golden.
   When behaviour *should* change, the golden is regenerated **in the same diff**
   so the review sees the before/after, never a silently-updated baseline.

## Harness

A pack wires this to its real test runner; the role is fixed:

```
# run the surface's tests + golden diffs, exit non-zero on any mismatch
<test-runner> <pure-calc test target>
```

- **No network.** Pure-calc takes no `[network]` set — if the "pure" change
  reaches for I/O, it was mis-classified; stop and re-run `intent-acceptance`.
- **Golden updates are reviewed, not trusted.** A regenerated golden is a code
  change: it appears in the `diff` evidence, and an unexplained golden churn is a
  red flag, not a pass.
- **Determinism is the contract.** A flaky golden run means the function is not
  actually pure (hidden ordering, locale, float formatting) — fix the
  non-determinism, do not retry until green.

## Evidence it produces

Feeds the `evidence-gate` for the `pure-calc` surface — `test-run` + `diff`:

```yaml
evidence:
  - kind: test-run
    surface: pure-calc
    summary: <e.g. "all 14 parser tests + 6 golden fixtures pass">
    detail: |
      $ <test-runner> <target>
      <output, exit status 0 visible>
  - kind: diff
    surface: pure-calc
    summary: <one line: what changed (incl. any regenerated golden)>
    detail: |
      <unified diff / key hunks>
```

A green `golden-tests` run over a `pure-calc`-only change is exactly the
condition the classifier's `auto-merge` autonomy requires: the deliverable can
finalize without a human, because the golden run already is the verification.
