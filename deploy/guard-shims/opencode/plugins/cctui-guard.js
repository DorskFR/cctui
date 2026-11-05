// OpenCode loads every export of a plugin file as a plugin: keep helpers private.

const CLAUDE_TOOL_NAMES = {
  bash: "Bash",
  edit: "Edit",
  glob: "Glob",
  grep: "Grep",
  list: "LS",
  multiedit: "MultiEdit",
  read: "Read",
  task: "Task",
  todoread: "TodoRead",
  todowrite: "TodoWrite",
  webfetch: "WebFetch",
  write: "Write",
};

const FAIL = "— failing closed";

function toolInput(args) {
  if (args === null || typeof args !== "object" || Array.isArray(args)) {
    return {};
  }
  if (typeof args.filePath === "string" && args.file_path === undefined) {
    return { ...args, file_path: args.filePath };
  }
  return args;
}

function timeoutMs() {
  const secs = Number(process.env.CCTUI_GUARD_TIMEOUT);
  return Number.isFinite(secs) && secs > 0 ? secs * 1000 : 10000;
}

async function denial(tool, args) {
  const base = process.env.CCTUI_GUARD_URL;
  if (!base) {
    return `cctui-guard plugin: CCTUI_GUARD_URL is unset ${FAIL}`;
  }
  if (typeof tool !== "string" || tool === "") {
    return `cctui-guard plugin: tool call without a name ${FAIL}`;
  }
  let res;
  try {
    res = await fetch(`${base.replace(/\/+$/, "")}/check?dialect=opencode`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        tool_name: CLAUDE_TOOL_NAMES[tool] ?? tool,
        tool_input: toolInput(args),
      }),
      signal: AbortSignal.timeout(timeoutMs()),
    });
  } catch (e) {
    return `cctui-guard plugin: guard unreachable at ${base} (${e?.message ?? e}) ${FAIL}`;
  }
  if (!res.ok) {
    return `cctui-guard plugin: guard answered HTTP ${res.status} ${FAIL}`;
  }
  let verdict;
  try {
    verdict = await res.json();
  } catch {
    return `cctui-guard plugin: invalid guard response ${FAIL}`;
  }
  if (verdict?.allow === true) {
    return null;
  }
  if (verdict?.allow === false) {
    return typeof verdict.reason === "string" && verdict.reason !== ""
      ? verdict.reason
      : "cctui-guard: blocked";
  }
  return `cctui-guard plugin: invalid guard response ${FAIL}`;
}

export const CctuiGuard = async () => ({
  "tool.execute.before": async (input, output) => {
    let reason;
    try {
      reason = await denial(input?.tool, output?.args);
    } catch (e) {
      reason = `cctui-guard plugin: ${e?.message ?? e} ${FAIL}`;
    }
    if (reason !== null) {
      throw new Error(reason);
    }
  },
});
