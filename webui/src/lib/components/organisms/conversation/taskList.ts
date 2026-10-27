import type { AgentEvent } from '@bindings/AgentEvent';
import type { TodoEntry } from '@bindings/TodoEntry';
import type { TodoItem, TodoStatus } from './types';

const text = (v: string | null | undefined) => (v?.trim() ? v : undefined);

const status = (raw: string): TodoStatus =>
	raw === 'completed' || raw === 'in_progress' ? raw : 'pending';

// Claude Code's task list as the server stores it. Null for a `TodoWrite` /
// `update_plan` list, whose entries carry no id and stay on the live stream.
export function taskTodos(entries: readonly TodoEntry[] | null | undefined): TodoItem[] | null {
	if (!entries?.length || !entries.some((e) => e.id)) return null;
	return entries.map((e) => ({
		content: e.content,
		status: status(e.status),
		activeForm: text(e.active_form),
		blockedBy: e.blocked_by?.length ? e.blocked_by.map((id) => `#${id}`) : undefined,
		id: text(e.id),
		description: text(e.description),
		owner: text(e.owner)
	}));
}

// The list the drawer shows: a stored task list unless a live `TodoWrite` /
// `update_plan` came after it.
export function drawerTodos(
	stored: readonly TodoEntry[] | null | undefined,
	live: TodoItem[] | null,
	liveIsNewer: boolean
): TodoItem[] | null {
	return (liveIsNewer ? null : taskTodos(stored)) ?? live;
}

// A step of the task list, which the server folds into its stored copy.
export function isTaskEvent(ev: AgentEvent): boolean {
	if (ev.type === 'tool_call') return ev.tool === 'TaskCreate' || ev.tool === 'TaskUpdate';
	return ev.type === 'tool_result' && /^\s*Task #\d+ created successfully: /.test(ev.output_summary);
}
