---
name: evidence-gate
description: Collect the evidence[] that proves a dispatched change is done — test-run output, the diff, screenshots/transcripts, coverage delta — keyed to the surface classes the change touched, and refuse to finalize (open the PR / report success) until every required surface has evidence. Use this as the final step of any dispatched implementation task, after the change is made and before the finalize/open-PR transition. The evidence is rendered on the PR body so human review is a glance, not a re-run.
---

# evidence-gate

The gate that turns "done" from an **assertion** into **proof**. The agent
reports success at the end of a run; without evidence a confident misread or a
half-working change sails through to a PR, and the human has to re-run the app to
find out. "Evidence, not assertions": the deliverable must *show* that each
acceptance condition holds, and the finalize transition is refused until it does.

Run this **once**, as the final step of an implementation task, after the change
is made and the tests are run, and **before** you open the PR / report
`status: "success"`.

## What you produce

A single **`evidence[]` array** attached to the result. Each entry is a typed,
self-contained artifact a reviewer can read without re-running anything. An entry
is `{ kind, surface, summary, detail }`:

- `kind` — the evidence type (see the table below).
- `surface` — the surface class (from the Intent+Acceptance artifact) it proves.
- `summary` — one line: what it demonstrates ("all 14 parser tests pass",
  "checkout shows the new total $42.00").
- `detail` — the artifact itself or a link to it: the command + its output, a
  diff hunk, an image/video URL, a transcript, the coverage delta.

### Evidence kinds

| Kind | Is | Proves |
| --- | --- | --- |
| `test-run` | the command run + its full output (exit status visible) | the change behaves as specified under test |
| `diff` | the unified diff (or its key hunks) | exactly what changed, nothing stray |
| `screenshot` / `video` | image/recording of the UI behaviour | a `frontend` / `brand-visible` surface renders/behaves correctly |
| `transcript` | a request/response or integration log | an `external-api` / `webhook` / `payments` round-trip succeeded |
| `coverage` | the coverage delta for the touched code | the new behaviour is actually exercised |

## Required evidence per surface

Every surface in the Intent+Acceptance artifact dictates what evidence the gate
demands. The finalize transition is refused unless **each** touched surface has
its required kind(s):

| Surface | Required evidence |
| --- | --- |
| `pure-calc` | `test-run` (the deterministic fixture passes) + `diff` |
| `frontend` | `screenshot` or `video` of the behaviour + `diff` |
| `backend` | `test-run` (endpoint/job exercised) + `diff` |
| `external-api` | `transcript` of the real round-trip + `diff` |
| `webhook` | `transcript` of the inbound/outbound contract + `diff` |
| `payments` | `transcript` of the money-movement path (sandbox) + `diff` |
| `brand-visible` | `screenshot` of the rendered copy/email + `diff` |

`diff` is required for every surface — it is the cheapest evidence and bounds
what was touched. A `coverage` entry is encouraged wherever a `test-run` is
required but never substitutes for it.

## The gate

The gate is the mirror of the Intent+Acceptance gate at the other end of the run.
Drive it off the **same** artifact: the acceptance script is reused verbatim, and
each acceptance condition must be backed by at least one evidence entry whose
`summary` shows the condition observably met.

- **Refuse to finalize** — if any touched surface is missing its required
  evidence kind, or any acceptance condition has no backing entry, **do not**
  transition to open-PR / report success. Either gather the missing evidence or
  report `status: "needs_human"` carrying what is blocked. Reporting
  `status: "success"` without populated `evidence[]` is a contract violation.
- **Finalize** — once every surface's required evidence is present and every
  acceptance condition is backed, write the `evidence[]` into the result, render
  it on the PR body (one section per surface, the `summary` as the heading and
  the `detail` folded under it), and open the PR.

In a guarded prompt (see `prompts/`), express this as a dedicated final step
whose only `[transition]: Exit` (and whose `remote-write` / open-PR capability)
is reached after the evidence is assembled. The guard makes the gate structural:
the agent cannot push / open the PR from an earlier step, so the evidence step is
the only door to a finalized deliverable.

## Output shape

Emit the evidence as a fenced block so downstream tooling (and the PR renderer)
can lift it unchanged into the result and the PR body:

```yaml
evidence:
  - kind: test-run
    surface: pure-calc
    summary: <one line: what it demonstrates>
    detail: |
      $ <command>
      <output, exit status visible>
  - kind: diff
    surface: pure-calc
    summary: <one line>
    detail: |
      <unified diff / key hunks>
```

Keep each entry self-contained: a reviewer reads `evidence[]` top to bottom and
is convinced, without checking out the branch or running the app.
