// Shared spawn-form types, extracted from SpawnModal (no behavior change).
import type { PermissionMode } from '@bindings/PermissionMode';

// "machine" = spawn on an enrolled daemon; "dispatch" = hand off to a k8s
// dispatcher (claude-worker) that runs the session in an ephemeral pod.
export type Target = 'machine' | 'dispatch';

export interface Form {
	machine_id: string;
	adapter_id: string;
	working_dir: string;
	name: string;
	prompt: string;
	permission_mode: PermissionMode;
	// dispatch-only fields (forwarded to the dispatcher as `payload`).
	dispatcher: string;
	identity: string;
	repo: string;
	ticket: string;
	prompt_file: string;
	// Model family is per-adapter (claude families vs codex models), like effort
	// below, so each gets its own field and they survive an adapter switch
	// (CCT-274). Dispatch (k8s) runs a claude worker → model_claude.
	model_claude: string;
	model_codex: string;
	// Named OAuth account to run under (CCT-237), resolved per-adapter at spawn.
	// Empty = no gateway injection (the worker's own auth).
	account: string;
	// Effort is per-adapter (claude and codex have different level sets), so each
	// gets its own slider + form field and they're preserved across an adapter
	// switch. Dispatch (k8s) runs a claude worker → uses effort_claude.
	effort_claude: string;
	effort_codex: string;
	timeout: string;
}
