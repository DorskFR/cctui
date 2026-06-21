---
name: comment-handling
description: Handle review comments on an open PR without capitulating. Classify each inbound comment as mechanical/objective (auto-fix) or judgment/subjective (PROPOSE or DEFEND, never silently comply), then act per class — fix mechanical ones, answer judgment ones with a reasoned response, and escalate needs_human when a thread is unresolved. Use this when a review comment / review-submitted event arrives on a PR the agent authored. Deterministic classifier; the defend-don't-cave rule protects quality against pushback.
---

# comment-handling

The run does not end at "PR opened". A reviewer (human or bot) leaves comments,
and the agent has to act on them — but the dangerous failure mode here is not
ignoring comments, it is **capitulating** to them. LLMs flip a correct answer to
an incorrect one under mild pushback at a measurable rate (~15% in published
sycophancy evals; the *format* of the objection — confident tone, authority
framing — drives the cave more than its substance). An agent that rewrites good
code because a comment was raised is churning quality away, not improving it.

This skill is the inbound mirror of `evidence-gate`: the gate proved the
deliverable was right when the PR opened; this skill keeps it right while the PR
is reviewed. It runs **per inbound comment** and is deterministic on the
comment's class — the agent cannot talk itself into "just make the change" for a
judgment comment any more than it could talk itself into a softer merge gate.

## Step 1 — classify the comment (deterministic)

Every inbound comment lands in exactly one class. Classification is a pure
function of *what the comment asks for*, never of who raised it or how forcefully:

| Class | What it is | Examples | Action |
| --- | --- | --- | --- |
| `mechanical` | objective, checkable, one right answer | lint/format, rename, dead-code removal, typo, a **demonstrable** bug (failing case named), a missing null-check the comment shows triggering | **auto-fix** |
| `judgment` | subjective, taste, scope, trade-off | "is this worth it", architecture/abstraction choice, naming *style* (vs a wrong name), "I'd have done it differently", premature-optimization claims, scope-creep requests | **propose or defend — never silently comply** |
| `unclear` | cannot tell what is being asked, or asks for something the artifact's Intent rules out | vague "this feels off", a change that contradicts the ratified Acceptance | **ask / escalate** |

A comment that *asserts* a bug but names no failing case is `judgment` (defend by
asking for the repro), not `mechanical` — "there might be a race here" is a
hypothesis to answer, not a defect to patch. Promote it to `mechanical` only once
a concrete failing case exists. When a comment bundles several asks, split it and
classify each part on its own line.

## Step 2 — act per class

### `mechanical` → auto-fix

Make the fix, re-run the surface's oracle (from the Step 2 classification plan)
so the change stays backed by evidence, and reply on the thread with the commit
that addresses it. A mechanical fix is in-scope by definition; it does not need a
new ratify round-trip. Resolve the thread.

### `judgment` → defend, don't cave

Do **not** edit code first. Write a reasoned reply that either:

- **DEFENDS** the current code — state *why* it is the way it is (tie back to the
  ratified Intent/Acceptance and the evidence), and ask the reviewer to confirm
  or counter. The default for a judgment comment on code that already passed the
  evidence gate is to **hold**, not to change. Agreement requires a *reason that
  survives the artifact*, not the mere existence of the objection.
- **PROPOSES** a change — only when the comment surfaces something the artifact
  genuinely missed. A proposal that alters scope or surfaces re-opens the
  `intent-acceptance` gate (the spec changed); it does not get silently absorbed.

Never rewrite working, evidence-backed code because a comment was *raised*. The
human merge gate already caps blast radius — a capitulating agent still cannot
merge — so the only thing the agent can damage by caving is quality. Hold the
line; let the human decide if the disagreement persists.

### `unclear` → ask / escalate

Reply asking the one question that would disambiguate. If the comment
contradicts the ratified Acceptance, do not resolve it by changing code — that is
a spec dispute, not a fix. Route `needs_human` with the conflict.

## Step 3 — converge or escalate

Re-run after each new comment. A thread is **resolved** when its mechanical fix
landed (oracle green) or the reviewer accepted the defense/proposal. When a
judgment thread stays open after one reasoned round-trip — reviewer presses, agent
has already given its reason — **stop**: report `status: "needs_human"` carrying
the open thread and both positions. Do not enter a churn loop of edit/revert under
repeated pushback; an unresolved judgment call is a human decision, by design.

## Invariants

- **The human merge gate is never bypassed.** This skill answers and fixes; it
  does not merge. Autonomy is whatever the Step 2 plan granted.
- **No silent compliance on judgment.** Every `judgment` comment gets a reply
  (defense or proposal) before any code moves; code moves only on `mechanical` or
  on a proposal the human ratified.
- **Mechanical fixes stay evidence-backed.** Re-run the surface's oracle after a
  fix; a fix that breaks an oracle is itself a regression to report, not to push.
- **Deterministic on class.** Same comment → same class → same action path,
  independent of the reviewer's tone or authority.
