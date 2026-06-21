---
name: classify-surface
description: Turn the surfaces[] of a ratified Intent+Acceptance artifact into a conditional pipeline plan — which oracle skills run, the autonomy level (auto-merge on green vs mandatory human gate), the required evidence, and whether a brand/taste human sign-off is needed. Use this as the step BETWEEN intent-acceptance and implement: the surface classes are inputs, the plan is the output that conditions the rest of the run. Deterministic — same surfaces always yield the same plan.
---

# classify-surface

The pipeline is effectively linear, but **which** oracles run, **how**
autonomous the run may be, and **whether** a human must sign off all depend on
what the task touches. This skill is the conditional middle: it reads the
`surfaces[]` of the ratified Intent+Acceptance artifact (never re-infers them)
and emits a **classification plan** that conditions Steps 2–3.

It is a pure function of the surface set — no judgement, no I/O. The same
`surfaces[]` always produce the same plan, so the routing is auditable and the
agent cannot talk itself into a softer gate. Run it **once**, after
`intent-acceptance` has produced (and, where required, the human has ratified)
the artifact, and **before** implementation begins.

## What you produce

A single **classification plan**, derived deterministically from `surfaces[]`:

```yaml
oracles: [<skill>, ...]        # the oracle skills the surfaces demand
autonomy: <auto-merge|human-gate>
brand_gate: <true|false>       # a brand-visible surface forces a human taste sign-off
required_evidence: [<kind>, ...]   # union over surfaces (drives the evidence-gate)
```

`oracles` is what gets verified, `autonomy` is how the merge is allowed to
happen, `brand_gate` is the one thing that cannot be oracle'd — only routed.

## Surface → plan table

Every surface class maps to a fixed row. A task that touches more than one
surface takes the **union** of the oracle/evidence columns and the **strictest**
autonomy (any `human-gate` surface forces `human-gate` overall).

| Surface | Oracles | Autonomy | Brand gate | Required evidence |
| --- | --- | --- | --- | --- |
| `pure-calc` | `golden-tests` | `auto-merge` | no | `test-run`, `diff` |
| `frontend` | `render-check` | `human-gate` | no | `screenshot`, `diff` |
| `backend` | `endpoint-tests` | `human-gate` | no | `test-run`, `diff` |
| `external-api` | `roundtrip-check` | `human-gate` | no | `transcript`, `diff` |
| `webhook` | `contract-check` | `human-gate` | no | `transcript`, `diff` |
| `payments` | `roundtrip-check`, `contract-check` | `human-gate` | no | `transcript`, `diff` |
| `brand-visible` | — | `human-gate` | **yes** | `screenshot`, `diff` |

The oracle names are pack-defined verification skills; a real pack wires them to
its actual test/render/round-trip tooling. The fixture lists them by role.

## The two routing decisions

### Autonomy

- **`auto-merge`** — granted **only** when *every* surface is `pure-calc`. A
  deterministic, oracle-backed change (golden tests over a pure function) can
  merge on green without a human in the loop: the oracle *is* the reviewer.
- **`human-gate`** — forced by **any** surface that moves money, touches auth /
  migrations, calls an external party, or renders something a customer sees. The
  oracle can prove behaviour but not that the behaviour is *wanted*; a human
  must sign off before merge. This mirrors the ratify gate's blast-radius split:
  `auto-merge` ⇔ `blast_radius: low`.

State `payments`, auth, and migrations explicitly as **always** `human-gate`
regardless of how green the oracles are — these are the surfaces where a silent
auto-merge is most dangerous.

### Brand / taste gate

`brand_gate: true` whenever a `brand-visible` surface is present — copy, layout,
pricing, naming, emails: anything an end customer reads. **Taste cannot be
oracle'd, only routed.** No test asserts that wording is on-brand or a price is
right, so the change routes to a human for a sign-off that is explicitly *not* a
correctness check. The brand gate is **independent** of and additional to the
autonomy gate: a brand-visible change is both `human-gate` *and* `brand_gate`,
and the human sign-off is a taste review, not a code review.

## How the plan conditions the rest of the run

- **Oracles** — Step 2 runs exactly the `oracles[]` skills, no more, no fewer.
- **Required evidence** — feeds the `evidence-gate` (Step 3): the union here is
  the minimum the gate demands before finalize.
- **Autonomy** — `auto-merge` lets Step 3 finalize on green oracles without a
  human round-trip; `human-gate` routes the finalized PR to a human and reports
  `status: "needs_human"` for the merge decision (the PR is open, the merge is
  not auto). The guard enforces this: an `auto-merge` capability is granted in
  the final step **only** when the plan says so.
- **Brand gate** — when `true`, the run additionally routes the rendered
  copy/layout to a human taste sign-off via the `needs_human` callback,
  carrying the brand-visible evidence, independent of the merge decision.

## Output shape

Emit the plan as a fenced block so the guard and downstream steps lift it
unchanged:

```yaml
plan:
  oracles: [golden-tests]
  autonomy: auto-merge
  brand_gate: false
  required_evidence: [test-run, diff]
```

Keep it terse and mechanical. The plan is read by the guard (to grant/withhold
the `auto-merge` capability and route the brand gate) and by a human in seconds.
Two tasks of different surface classes will, by construction, follow different
oracle/autonomy/brand paths — automatically, from the same artifact.
