#!/usr/bin/env bash
# codex-run — one-shot `codex exec` inside a hardened cctui worker pod (CCT-526).
#
# Model, reasoning effort, approval policy and sandbox mode all come from
# ~/.codex/config.toml, which the worker entrypoint (phase_codex_config) pins
# per-pod (model/effort from TASK_MODEL/TASK_EFFORT; approval_policy = "never" +
# sandbox_mode = "danger-full-access"). So this wrapper carries only what config
# CANNOT express:
#
#   * --skip-git-repo-check — CLI-only, no config.toml equivalent, and required
#     because the workspace often isn't a git repo (codex exec exits 1 otherwise);
#   * closes stdin (`codex exec` blocks on it otherwise);
#   * bounds the run with `timeout` (default 500s, override CODEX_TIMEOUT).
#
# It deliberately does NOT pass --sandbox: the pod is ALREADY a hardened sandbox
# (Landlock + seccomp + guard-proxy egress), and codex's own inner sandbox is
# bubblewrap, which the worker seccomp filter blocks — so a --sandbox flag makes
# every file read fail and codex returns nothing. Full-access + no-approval come
# from config.toml, not a flag here.
#
# Usage:
#   codex-run "prompt text"                 # prompt as an argument
#   codex-run -f prompt.md                  # prompt read from a file
#   codex-run -f "$PROMPT_FILE" > out 2>&1  # capture output, then POLL the file
#
# Output goes to stdout/stderr. When capturing to a file, POLL the file/process
# — never `tail -f` it, which buffers until codex exits. Extra args after the
# prompt are forwarded to `codex exec` verbatim (e.g. an ad-hoc `-c key=val`).
set -euo pipefail

_prompt=
case "${1:-}" in
    -f)
        [ -n "${2:-}" ] || { echo "codex-run: -f needs a file path" >&2; exit 2; }
        [ -f "$2" ] || { echo "codex-run: no such prompt file: $2" >&2; exit 2; }
        _prompt=$(cat "$2")
        shift 2
        ;;
    "")
        echo "codex-run: need a prompt (an argument or -f FILE)" >&2
        exit 2
        ;;
    *)
        _prompt=$1
        shift
        ;;
esac

exec timeout "${CODEX_TIMEOUT:-500}" codex exec \
    --skip-git-repo-check \
    "$@" "$_prompt" < /dev/null
