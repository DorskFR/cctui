# Example dispatch prompt (guarded, two steps)

A neutral example showing the prompt-step format `cctui-guard` parses. Set
`TASK_PROMPT_FILE=example-task.md` to dispatch it; it resolves under
`/opt/context/prompts/`. The `[allowed]`/`[network]` lines reference the sets
defined in `../guard-rules.md`.

# Step 1: Research the task

Explore the workspace; do not modify anything. Read the relevant code and the
referenced docs, then transition to Step 2 when you understand the change.

[allowed]: all-read
[disallowed]: *
[network]: net-model, net-vcs
[transition]: 2, Exit

# Step 2: Implement

Make the change, run the tests, and commit. Push only when the work is complete.

[allowed]: all-read, code-write, Bash, git commit
[disallowed]: remote-write
[network]: net-model
[transition]: Exit
