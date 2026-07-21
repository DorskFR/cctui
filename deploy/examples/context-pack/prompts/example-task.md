# Example dispatch prompt (guarded, five steps)

A neutral example showing the prompt-step format `cctui-guard` parses. Set
`TASK_PROMPT_FILE=example-task.md` to dispatch it; it resolves under
`/opt/context/prompts/`. The `[allowed]`/`[network]` lines reference the sets
defined in `../guard-rules.md`.

The first step is an **Intent+Acceptance ratify gate** (see the
`intent-acceptance` skill): the agent emits what "done" means, the concrete
success condition, and which surfaces the change touches; the human ratifies it
cheaply, and only then does the transition unlock. The gate is structural — the
guard will not let Step 1 reach implementation without passing through it, so a
misread is caught for ~30s of human attention instead of at PR time.

The second step is the **conditional classifier** (see the `classify-surface`
skill): it turns the ratified `surfaces[]` into a pipeline plan — which oracle
skills run, the autonomy level (`auto-merge` on green for pure-calc, mandatory
human gate for payments/auth/migrations and any customer-visible surface), the
required evidence, and whether a brand/taste human sign-off is needed. The plan
is a pure function of the surface set, so two tasks of different surface classes
follow different oracle/autonomy/brand paths automatically.

The third step implements, running exactly the oracle skills the plan selected
(see the per-surface oracle skills — `golden-tests`, `render-check`,
`endpoint-tests`, `roundtrip-check`, `contract-check`). Each exercises its
surface in-pod against test-mode / replay and is granted only that surface's
sandbox net-allow — never a third party's production host. Before introducing
any new abstraction it runs the **prior-art** step (cite the existing
util/type/component you'll reuse, or justify a new one — the retrieval half of
the consistency problem), and the `[gate]` runs the **consistency-gates**
(ast-grep / dependency-cruiser / jscpd) so a correct-but-inconsistent change
cannot transition to finalize; a recurring violation is codified as a one-rule
change (the ratchet) in the same diff.

The last step is the mirror of the ratify gate — an **evidence-required "done"
gate** (see the `evidence-gate` skill): the agent must assemble an `evidence[]`
array (keyed to the plan's `required_evidence`) proving each acceptance
condition before finalize unlocks. `remote-write` is only granted in the final
step, and the `auto-merge` capability is granted there **only** when the plan
says `autonomy: auto-merge`; otherwise the finalized PR routes to a human via
`needs_human`. A `brand_gate: true` plan additionally routes the rendered
copy/layout to a human taste sign-off. Human review becomes a glance at the
evidence, not a re-run of the app.

Between finalize and review, a **deliverable-acceptance agent** (see the
`acceptance-agent` skill) re-confirms the change end-to-end against the *deployed*
PR. It runs in a **separate, clean context** — handed the ratified Intent+
Acceptance success condition but never the diff or the implementer's evidence — so
it cannot mark its own homework. It drives the per-PR preview env (Playwright for
a UI surface, HTTP for an API surface) and attaches an independent `pass|fail`
verdict; a `fail` blocks merge regardless of what the implementer asserted. It is
not a guarded step in *this* prompt — it is a distinct dispatch granted only the
`net-preview` net-allow (plus `net-model`) and no code-write / remote-write
capability; standing up the preview env it drives is infra, out of scope for the
pack.

The fifth step handles **inbound review comments** without capitulating (see the
`comment-handling` skill): each comment is classified — `mechanical` (auto-fix,
re-run the oracle, keep it evidence-backed) vs `judgment` (propose or defend,
never silently rewrite good code) vs `unclear` (ask / escalate) — and an
unresolved judgment thread escalates `needs_human` rather than churning. The
human merge gate from Step 4 still holds, so this step answers and fixes but
never merges.

# Step 1: Intent + Acceptance (ratify before implement)

Gather context — read the task, the referenced docs, and the relevant code; do
not modify anything. Then run the `intent-acceptance` skill to emit the
Intent+Acceptance artifact: what "done" means, the concrete success
condition(s), and which surfaces the change touches.

Route the artifact to the human to ratify or correct (the `needs_human`
callback) UNLESS every surface is low blast radius, in which case auto-ratify.
Only transition to Step 2 once the artifact exists and is ratified. Persist it —
it is reused verbatim as the acceptance script in Step 4.

[allowed]: all-read
[disallowed]: *
[network]: net-model, net-vcs
[transition]: 2, Exit

# Step 2: Classify (select oracle-set, autonomy, brand gate)

Run the `classify-surface` skill against the ratified `surfaces[]` from Step 1 —
do not re-infer the surfaces. Emit the classification plan: the `oracles[]` the
surfaces demand, the `autonomy` level (`auto-merge` only when every surface is
`pure-calc`; `human-gate` for payments/auth/migrations and any customer-visible
surface), `brand_gate` (true iff a `brand-visible` surface is present), and the
union `required_evidence`. Persist the plan — it conditions Steps 3 and 4. Do not
modify anything in this step.

[allowed]: all-read
[disallowed]: *
[network]: net-model
[transition]: 3, Exit

# Step 3: Implement

Make the change and run exactly the `oracles[]` the Step 2 plan selected — no
more, no fewer (see the per-surface oracle skills: `golden-tests`,
`render-check`, `endpoint-tests`, `roundtrip-check`, `contract-check`). Each
oracle exercises its surface in-pod against test-mode / replay and emits the
evidence Step 4 consumes. The network below grants only the per-surface sandbox
sets — never a third party's production host; a surface that needs a host its set
does not grant was mis-classified. Commit. Push only in Step 4.

**Before** you write the first new utility/type/component, run the `prior-art`
skill: search the generated helper index (`docs/helper-index.md`) for an existing
one and cite the prior art you'll reuse, or justify a new one. The retrieval gate
catches reinvention up front, where it is cheapest.

The `[gate]` below is a **deterministic transition gate** (guard hardening): the
transition into Step 4 will not fire until `make oracle-check` exits 0, so the
finalize step cannot be reached on a claim of "done" — the gate is the proof. The
same target chains the **consistency-gates** (`make consistency-check` —
ast-grep / dependency-cruiser / jscpd), so a change that is correct but
*inconsistent* (banned API, broken layer boundary, copy-paste) also cannot
transition; fix the violation rather than suppress it, and codify a recurring one
as a one-rule change in the same diff (the ratchet). A real pack points the gate
at whatever command runs its selected oracles + consistency gates green (here a
placeholder `make` target). On entry to each step the guard re-injects this
step's prompt verbatim, so a long run re-anchors on the trusted instructions
rather than its own drifting summary. Add a `[compact]` line to a step to also
ask the agent to trim its working context (off by default — leave it out on
large-context models where compaction loses more than it saves).

[allowed]: all-read, code-write, Bash, git commit
[disallowed]: remote-write
[network]: net-model, net-external-sandbox, net-payments-sandbox
[compact]: false
[gate]: make oracle-check
[transition]: 4, Exit

# Step 4: Accept (evidence-required done gate)

Run the deployed change against the Acceptance section of the Step 1 artifact —
verbatim. Then run the `evidence-gate` skill: assemble the `evidence[]` array —
keyed to the Step 2 plan's `required_evidence` — with one entry backing each
acceptance condition. Refuse to finalize (report `status: "success"`) until
every surface has its required evidence; otherwise report `status:
"needs_human"` with what is blocked.

Before writing `RESULT_FILE`, validate the result envelope against the pack's
per-flow schema `~/.claude/schemas/result.json` (also at
`/opt/context/schemas/result.json`) — e.g.
`check-jsonschema --schemafile ~/.claude/schemas/result.json "$RESULT_FILE"`, or
any JSON-schema validator. A result that does not conform (missing `status`, an
unknown `status`, an empty `evidence[]` on a code-touching success) is a contract
violation — fix the envelope, do not write it.

Once `evidence[]` is populated and every acceptance condition is observably met,
render the evidence on the PR body and open the PR. Then honor the plan's
routing: if `autonomy: auto-merge` (every surface `pure-calc`, oracles green),
finalize the merge; otherwise leave the PR for a human and report `status:
"needs_human"` for the merge decision. If `brand_gate: true`, additionally route
the rendered copy/layout to a human taste sign-off via `needs_human`, carrying
the brand-visible evidence.

The `[llmjudge]` below is the **semantic acceptance gate** (guard hardening,
CCT-516) — the mirror of Step 3's deterministic `[gate]`. Where the gate proves
mechanical facts (tests exit 0), the judge answers binary questions about the
*meaning* of the evidence, in a clean context (the ratified artifact, the
assembled `evidence[]`, and the diff — never this session's reasoning), and the
transition out of this step is refused unless **every** question scores 1. Its
per-question verdicts come back as a `kind: "judge"` evidence entry — attach it
to the result callback's `evidence[]` so the PR carries e.g. "5/6 verified; Q4
FAILED: no test covers replayed delivery".

[allowed]: all-read, remote-write, Bash
[disallowed]:
[network]: net-model, net-vcs
[llmjudge]
- Does every acceptance condition in the ratified Step 1 artifact have at least one evidence[] entry that observably backs it? :: evidence[] covers only two of three conditions
- Does the diff itself implement each acceptance condition, rather than only adding tests, docs, or scaffolding? :: the PR only adds a test asserting the current behavior
- Was every oracle the Step 2 plan selected actually run, with its output captured in evidence[]? :: the plan requires endpoint-tests but no endpoint output appears
- Is each evidence[] entry backed by a command, artifact, or observable output rather than a bare assertion? :: an entry says "verified manually" with nothing attached
[transition]: 5, Exit

# Step 5: Address review comments (classify, defend-don't-cave)

Run **once per inbound review comment** on the open PR (see the
`comment-handling` skill). Classify each comment deterministically on *what* it
asks for, not who raised it or how forcefully:

- `mechanical` (lint, rename, dead code, a **demonstrated** bug) → fix it, re-run
  the surface's oracle so the fix stays evidence-backed, reply with the commit,
  resolve the thread.
- `judgment` (taste, scope, architecture, "is this worth it") → **propose or
  defend, never silently comply**. Reply with a reason tied to the ratified
  Intent/Acceptance and the evidence; the default is to hold. A change that
  alters scope/surfaces re-opens Step 1, it is not absorbed silently.
- `unclear` / contradicts the ratified Acceptance → ask the disambiguating
  question, or report `status: "needs_human"` for the spec dispute.

Do not enter an edit/revert loop under repeated pushback: after one reasoned
round-trip on an open judgment thread, report `status: "needs_human"` carrying
both positions. The human merge gate from Step 4 is never bypassed — this step
answers and fixes, it does not merge.

[allowed]: all-read, code-write, Bash, git commit, github-write
[disallowed]:
[network]: net-model, net-vcs, net-external-sandbox, net-payments-sandbox
[transition]: Exit
