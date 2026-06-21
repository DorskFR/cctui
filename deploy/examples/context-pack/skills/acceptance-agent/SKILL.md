---
name: acceptance-agent
description: Drive a DEPLOYED preview of the change against the Intent+Acceptance success condition from a clean context — Playwright for a frontend surface, HTTP for a backend/API surface — and attach a pass/fail verdict with evidence. Use this as a SEPARATE-context check after the implementation PR is opened and deployed to its per-PR preview environment, never in the implementer's own session. It re-derives the test from the ratified success condition alone (not the diff) so it cannot mark its own homework.
---

# acceptance-agent

The one oracle neither the guard nor the cross-model review covers: **does the
change actually do the intended thing, end to end, in a running deployment.**
Today a human fills that gap by checking out the branch and exercising it in the
dev stack — the primary bottleneck in both the dispatch and the work→PR flows.
This skill moves that check into the pipeline as attached evidence, so the human
reviews a verdict instead of re-running the app.

It is the deployed-end mirror of the two bracketing gates: `intent-acceptance`
states the success condition up front, `evidence-gate` makes the *implementer*
prove it at finalize, and this agent **independently re-confirms it against the
live deployment** — closing the loop the implementer cannot close for itself.

## Why a separate context

The implementer evidence (`evidence-gate`) is necessary but not sufficient: it is
produced by the same context that wrote the code, so it shares that context's
blind spots and its incentive to declare success. An agent that grades its own
work grades to pass. This step is therefore run in a **clean context that never
saw the implementation**:

- It is given the **Intent+Acceptance artifact only** — the `intent:` and
  `acceptance:` conditions and the `surfaces[]`. It is **not** given the diff,
  the implementer's transcript, or the implementer's `evidence[]`.
- It re-derives a concrete test plan from each acceptance condition by itself,
  then executes it against the **deployed preview**, not the source tree.
- It cannot push, edit code, or open/merge a PR. Its only outputs are a verdict
  and evidence. A failing verdict routes back; it never "fixes" to make itself
  pass.

Separating the grader from the author is the whole point — it is the structural
reason this verdict is worth more than the implementer's self-assertion.

## Input: the deployed preview + the success condition

Two things are handed in:

1. **A per-PR preview deployment** — an isolated, ephemeral environment running
   *this PR's* build, reachable at a `PREVIEW_BASE_URL` (and, for an API
   surface, with whatever test-mode credentials the surface needs). Standing up
   that environment is **infra** (k8s + ArgoCD), out of scope for this skill —
   see *Preview environments* below; the skill assumes the URL is provided.
2. **The ratified Intent+Acceptance artifact** — verbatim, reused as the
   acceptance script. The `acceptance[]` conditions are the assertions; the
   `surfaces[]` decide the driver.

If no `PREVIEW_BASE_URL` is provided, the deliverable cannot be accepted
end-to-end from a running deployment — report `status: "needs_human"` carrying
that the preview env is missing, do not fall back to grading the source tree.

## Driver, per surface

The surface class chosen up front decides how the deployed change is exercised.
Each driver reaches **only** the preview deployment (and a surface's sandbox
host) — never a third party's production host.

| Surface | Driver | Asserts the condition by |
| --- | --- | --- |
| `frontend` | Playwright against `PREVIEW_BASE_URL` | navigating + observing the UI ("clicking X shows Y"); capture a screenshot/video |
| `brand-visible` | Playwright against `PREVIEW_BASE_URL` | rendering the copy/email/layout; capture a screenshot |
| `backend` | HTTP against `PREVIEW_BASE_URL` | calling the endpoint/job and asserting status + response field ("returns 200 with field Z") |
| `external-api` | HTTP via the surface's sandbox | driving the round-trip in test-mode and asserting the contract; capture the transcript |
| `webhook` | replay against `PREVIEW_BASE_URL` | posting the recorded payload and asserting the handler's effect; capture the transcript |
| `payments` | HTTP via the payments sandbox | driving the money-movement path in sandbox and asserting the effect; capture the transcript |
| `pure-calc` | (no preview needed) | a `pure-calc`-only change has no deployed behaviour to drive — the `golden-tests` oracle + `evidence-gate` are sufficient; this step is skipped (see *When to run*) |

For each `acceptance[]` condition the agent writes the smallest script that
**observes** the condition met on the preview, runs it, and records the outcome.
A condition that cannot be expressed as an observation on the deployment was a
weak acceptance condition — report `needs_human` to tighten the spec rather than
guessing.

## When to run

Driven off the `intent-acceptance` `blast_radius`, the same lever the ratify gate
uses:

- **Run** when any surface is `frontend`, `backend`, `external-api`, `webhook`,
  `payments`, or `brand-visible` — i.e. anything with observable deployed
  behaviour (medium/high blast radius). These are exactly the classes a human
  used to check out and test by hand.
- **Skip** a `pure-calc`-only change: there is nothing deployed to drive, and the
  deterministic golden test the implementer already ran *is* the end-to-end
  proof. Record `verdict: skipped` with that reason.

## Output: a verdict + evidence

Emit a single fenced block — a per-condition verdict plus the evidence that backs
it, so the human (and the merge gate) reads a result, not a re-run. The evidence
entries are the same `{ kind, surface, summary, detail }` shape `evidence-gate`
uses, so they render on the PR body alongside the implementer's:

```yaml
acceptance_run:
  preview_url: <PREVIEW_BASE_URL>
  verdict: <pass|fail|needs_human|skipped>   # fail/needs_human blocks merge
  conditions:
    - condition: <the acceptance[] line, verbatim>
      result: <pass|fail>
      evidence:
        - kind: <screenshot|video|transcript|test-run>
          surface: <class>
          summary: <one line: what was observed on the preview>
          detail: |
            <the artifact: screenshot/video URL, request/response transcript,
            or the driver script + its output, exit status visible>
```

- **`pass`** — every condition observed met on the preview. Attach the verdict +
  evidence to the session and PR; the change is end-to-end confirmed and the
  human merge gate is now a glance, not a checkout.
- **`fail`** — a condition was not met. Attach the failing evidence (the
  screenshot/transcript showing the gap) and report `status: "needs_human"` (or
  route back to the implementer); **never** edit code to make the assertion pass.
- **`needs_human`** — the preview env is missing, or a condition is not
  observable as stated. Carry exactly what is blocked.
- **`skipped`** — `pure-calc`-only; nothing deployed to drive.

The verdict is **independent of, and composed with, the implementer's
`evidence-gate` output** — both must be green for an unattended merge; a `fail`
here blocks regardless of what the implementer asserted.

## Preview environments (infra — not this skill)

This skill assumes a deployed preview exists at `PREVIEW_BASE_URL`. **Standing up
a per-PR ephemeral preview environment is infra, not part of the context pack:**
it needs the cluster to deploy *this PR's* build to an isolated namespace on PR
open and tear it down on close (e.g. an ArgoCD `ApplicationSet` over a PR
generator, plus the dispatch that injects the resulting URL into this agent's
input). That machinery lives in the infra repo / the dispatch layer, never as an
in-repo default. The skill is the reusable, repo-resident half (the clean-context
driver + verdict contract) and works against any preview URL the pipeline hands
it; the environment-provisioning half is wired separately.
