# Example dispatch prompt (guarded, four steps)

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

The third step implements, running exactly the oracle skills the plan selected.

The last step is the mirror of the ratify gate — an **evidence-required "done"
gate** (see the `evidence-gate` skill): the agent must assemble an `evidence[]`
array (keyed to the plan's `required_evidence`) proving each acceptance
condition before finalize unlocks. `remote-write` is only granted in the final
step, and the `auto-merge` capability is granted there **only** when the plan
says `autonomy: auto-merge`; otherwise the finalized PR routes to a human via
`needs_human`. A `brand_gate: true` plan additionally routes the rendered
copy/layout to a human taste sign-off. Human review becomes a glance at the
evidence, not a re-run of the app.

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
more, no fewer. Commit. Push only in Step 4.

[allowed]: all-read, code-write, Bash, git commit
[disallowed]: remote-write
[network]: net-model
[transition]: 4, Exit

# Step 4: Accept (evidence-required done gate)

Run the deployed change against the Acceptance section of the Step 1 artifact —
verbatim. Then run the `evidence-gate` skill: assemble the `evidence[]` array —
keyed to the Step 2 plan's `required_evidence` — with one entry backing each
acceptance condition. Refuse to finalize (report `status: "success"`) until
every surface has its required evidence; otherwise report `status:
"needs_human"` with what is blocked.

Once `evidence[]` is populated and every acceptance condition is observably met,
render the evidence on the PR body and open the PR. Then honor the plan's
routing: if `autonomy: auto-merge` (every surface `pure-calc`, oracles green),
finalize the merge; otherwise leave the PR for a human and report `status:
"needs_human"` for the merge decision. If `brand_gate: true`, additionally route
the rendered copy/layout to a human taste sign-off via `needs_human`, carrying
the brand-visible evidence.

[allowed]: all-read, remote-write, Bash
[disallowed]:
[network]: net-model, net-vcs
[transition]: Exit
