import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import type { TodoEntry } from '@bindings/TodoEntry';
import { drawerTodos, isTaskEvent, taskTodos } from './taskList';

const call = (tool: string, input: unknown = {}) =>
	({ type: 'tool_call', tool, input, ts: 0 }) as AgentEvent;
const result = (output_summary: string) =>
	({ type: 'tool_result', tool: '', output_summary, error: false, ts: 0 }) as AgentEvent;

describe('taskTodos', () => {
	it('maps a stored task list onto drawer items', () => {
		const entries = [
			{ content: 'one', status: 'completed', id: '1' },
			{
				content: 'two',
				status: 'in_progress',
				active_form: 'Doing two',
				id: '2',
				description: 'why',
				blocked_by: ['1'],
				owner: 'agent-a'
			},
			{ content: 'three', status: 'weird', id: '3', description: '  ' }
		] as TodoEntry[];
		expect(taskTodos(entries)).toEqual([
			{ content: 'one', status: 'completed', id: '1' },
			{
				content: 'two',
				status: 'in_progress',
				activeForm: 'Doing two',
				id: '2',
				description: 'why',
				blockedBy: ['#1'],
				owner: 'agent-a'
			},
			{ content: 'three', status: 'pending', id: '3' }
		]);
	});

	it('leaves a TodoWrite list, which has no ids, to the live stream', () => {
		const todoWrite = [{ content: 'a', status: 'pending', active_form: 'A-ing' }] as TodoEntry[];
		expect(taskTodos(todoWrite)).toBeNull();
		expect(taskTodos([])).toBeNull();
		expect(taskTodos(undefined)).toBeNull();
	});
});

describe('isTaskEvent', () => {
	it('spots task steps and nothing else', () => {
		expect(isTaskEvent(call('TaskCreate', { subject: 's' }))).toBe(true);
		expect(isTaskEvent(call('TaskUpdate', { taskId: '1', status: 'completed' }))).toBe(true);
		expect(isTaskEvent(result('Task #3 created successfully: ship it'))).toBe(true);
		expect(isTaskEvent(call('TodoWrite', { todos: [] }))).toBe(false);
		expect(isTaskEvent(call('TaskList'))).toBe(false);
		expect(isTaskEvent(result('Updated task #3 status'))).toBe(false);
	});
});

describe('drawerTodos', () => {
	const stored = [{ content: 'task', status: 'pending', id: '1' }] as TodoEntry[];
	const live = [{ content: 'todo', status: 'pending' as const }];

	it('prefers the stored task list', () => {
		expect(drawerTodos(stored, live, false)?.[0].content).toBe('task');
	});

	it('lets a newer live TodoWrite win', () => {
		expect(drawerTodos(stored, live, true)).toBe(live);
	});

	it('keeps the live list when the stored one is a TodoWrite list', () => {
		const todoWrite = [{ content: 'stored todo', status: 'pending' }] as TodoEntry[];
		expect(drawerTodos(todoWrite, live, false)).toBe(live);
		expect(drawerTodos(undefined, null, false)).toBeNull();
	});
});
