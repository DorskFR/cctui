---
name: intent-acceptance
description: Emit an Intent+Acceptance artifact from gathered task context — what "done" means, the concrete success condition(s), and which surfaces the change touches — then route it to the human to ratify BEFORE implementation begins. Use this as the first step of any dispatched implementation task, after context gathering and before writing code. The same artifact is reused verbatim as the acceptance script at the end.
---

# intent-acceptance

The cheap gate that catches misunderstandings before code is written — the
"door in the garden" problem: the agent confidently builds the wrong thing and
the mistake is only caught at PR / dev-stack time, after the expensive work is
done. Surfacing the inferred intent for a ~30-second human confirmation up front
is orders of magnitude cheaper than discovering the misread at review.

Run this **once**, as the first step of an implementation task, after you have
gathered context (ticket, comments, linked discussion, the relevant code) and
**before** you touch any file.

## What you produce

A single **Intent+Acceptance artifact**: a short, structured statement, not
prose. It has exactly three parts.

1. **Intent** — one or two sentences: what "done" means, stated as the outcome
   the requester wants, not the steps you will take.
2. **Acceptance** — the concrete, checkable success condition(s). Each must be
   something a later acceptance agent (or a human) can verify by observation —
   "clicking X shows Y", "endpoint returns 200 with field Z", "the total for
   fixture F equals N". Avoid "the code is correct"; prefer "behaviour B is
   observable".
3. **Surfaces** — which of the fixed surface classes this change touches. The
   class drives how the change is later verified and how heavy the gate is.

### Surface classes

A small fixed set — tag every surface the change touches (a change may touch
more than one):

| Class | Means | Blast radius |
| --- | --- | --- |
| `pure-calc` | deterministic logic, no I/O (parsing, math, formatting) | low |
| `frontend` | user-visible UI behaviour / rendering | medium |
| `backend` | server logic, APIs, jobs | medium |
| `external-api` | calls a third-party API | high |
| `webhook` | inbound/outbound webhook contracts | high |
| `payments` | money movement / billing (e.g. Stripe) | high |
| `brand-visible` | copy, emails, anything an end customer reads | high |

## The ratify gate

The artifact must be **ratified by a human before implementation unlocks** —
this is the whole point of the step. Two paths, chosen by blast radius:

- **Auto-ratify** — if *every* surface the change touches is **low** blast
  radius (`pure-calc` only), record the artifact and proceed. The round-trip is
  not worth a human's attention for a pure refactor or a formatting fix.
- **Human ratify** — if *any* surface is medium or high blast radius, **pause
  and route the artifact to the human** for a quick ratify/correct, and do not
  transition into the implementation step until you have their confirmation. In
  a guarded dispatch this is the `needs_human` callback: emit a result with
  `status: "needs_human"` carrying the artifact, and wait. When the human
  corrects the intent, replace the artifact with the corrected version before
  proceeding — the correction is the spec.

In a guarded prompt (see `prompts/`), express this as a dedicated early step
whose only `[transition]` into the implement step is gated behind producing —
and, for non-low blast radius, ratifying — this artifact. The guard makes the
gate structural: the agent literally cannot enter the implement step first.

## Persist and reuse

Write the (ratified) artifact to the result/working dir so it survives the
session, and attach it to the session + PR. **Reuse it verbatim** at the end:
the Acceptance section is the script the deliverable-acceptance step runs the
deployed change against. Drafting it once and reusing it twice is deliberate —
the thing you promised up front is the thing you are checked against at the end.

## Output shape

Emit the artifact as a fenced block so downstream tooling can lift it
unchanged:

```yaml
intent: >
  <one or two sentences: the outcome that means "done">
acceptance:
  - <checkable success condition>
  - <checkable success condition>
surfaces: [<class>, ...]
blast_radius: <low|medium|high>   # max over surfaces
ratify: <auto|human>              # auto when blast_radius == low
```

Keep it terse. The artifact is read by a human in seconds and by an acceptance
agent at the end — both want signal, not narrative.
