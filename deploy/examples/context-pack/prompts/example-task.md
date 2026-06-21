# Example dispatch prompt (guarded, three steps)

A neutral example showing the prompt-step format `cctui-guard` parses. Set
`TASK_PROMPT_FILE=example-task.md` to dispatch it; it resolves under
`/opt/context/prompts/`. The `[allowed]`/`[network]` lines reference the sets
defined in `../guard-rules.md`.

The first step is an **Intent+Acceptance ratify gate** (see the
`intent-acceptance` skill): the agent emits what "done" means and the concrete
success condition, the human ratifies it cheaply, and only then does the
transition into the implement step unlock. The gate is structural — the guard
will not let Step 1 reach implementation without passing through it, so a
misread is caught for ~30s of human attention instead of at PR time.

# Step 1: Intent + Acceptance (ratify before implement)

Gather context — read the task, the referenced docs, and the relevant code; do
not modify anything. Then run the `intent-acceptance` skill to emit the
Intent+Acceptance artifact: what "done" means, the concrete success
condition(s), and which surfaces the change touches.

Route the artifact to the human to ratify or correct (the `needs_human`
callback) UNLESS every surface is low blast radius, in which case auto-ratify.
Only transition to Step 2 once the artifact exists and is ratified. Persist it —
it is reused verbatim as the acceptance script in Step 3.

[allowed]: all-read
[disallowed]: *
[network]: net-model, net-vcs
[transition]: 2, Exit

# Step 2: Implement

Make the change, run the tests, and commit. Push only when the work is complete.

[allowed]: all-read, code-write, Bash, git commit
[disallowed]: remote-write
[network]: net-model
[transition]: 3, Exit

# Step 3: Accept

Run the deployed change against the Acceptance section of the Step 1 artifact —
verbatim. Attach the evidence to the PR. Push only once every acceptance
condition is observably met.

[allowed]: all-read, remote-write, Bash
[disallowed]:
[network]: net-model, net-vcs
[transition]: Exit
