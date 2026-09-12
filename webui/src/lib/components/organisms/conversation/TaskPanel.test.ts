import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import TaskPanel from './TaskPanel.svelte';
import { taskPanelKey } from './taskPanel';
import type { TodoItem, TodoProgress } from './types';

let comp: ReturnType<typeof mount> | null = null;
beforeEach(() => localStorage.clear());
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	localStorage.clear();
});

const progressOf = (items: TodoItem[]): TodoProgress => ({
	items,
	done: items.filter((t) => t.status === 'completed').length,
	total: items.length,
	inProgress: items.find((t) => t.status === 'in_progress') ?? null
});

const sample = progressOf([
	{ content: 'Parse the payload', status: 'completed' },
	{ content: 'Render the panel', status: 'in_progress', activeForm: 'Rendering the panel' },
	{ content: 'Ship it', status: 'pending', blockedBy: ['Render the panel'] }
]);

const render = (progress: TodoProgress | null, sessionId = 's1') => {
	comp = mount(TaskPanel, { target: document.body, props: { sessionId, progress } });
};

const strip = () => document.querySelector('.strip') as HTMLButtonElement | null;
const text = () => document.body.textContent ?? '';

describe('TaskPanel', () => {
	it('is absent entirely when the session never produced a task list', () => {
		render(null);
		expect(document.querySelector('.tasks')).toBeNull();
		expect(text().trim()).toBe('');
	});

	it('is collapsed by default, showing only the strip', () => {
		render(sample);
		expect(strip()).not.toBeNull();
		expect(strip()?.getAttribute('aria-expanded')).toBe('false');
		expect(document.querySelector('.list')).toBeNull();
		expect(text()).toContain('1/3');
	});

	it('expands to the full list with subject, status and blocked-by relations', () => {
		render(sample);
		strip()?.click();
		expect(strip()?.getAttribute('aria-expanded')).toBe('true');
		expect(document.querySelectorAll('.task')).toHaveLength(3);
		expect(text()).toContain('Parse the payload');
		expect(text()).toContain('in progress');
		expect(text()).toContain('blocked by Render the panel');
	});

	it('omits the blocked-by chip when a task has no relations', () => {
		render(progressOf([{ content: 'lonely', status: 'pending' }]));
		strip()?.click();
		expect(document.querySelector('.blocked')).toBeNull();
	});

	it('remembers the expanded state per session', () => {
		render(sample, 'sA');
		strip()?.click();
		expect(localStorage.getItem(taskPanelKey('sA'))).toBe('1');

		unmount(comp!);
		comp = null;
		document.body.innerHTML = '';

		render(sample, 'sA');
		expect(strip()?.getAttribute('aria-expanded')).toBe('true');
	});

	it('does not carry one session’s expansion over to another', () => {
		localStorage.setItem(taskPanelKey('sA'), '1');
		render(sample, 'sB');
		expect(strip()?.getAttribute('aria-expanded')).toBe('false');
	});

	it('collapsing clears the persisted flag rather than storing a falsy value', () => {
		render(sample, 'sA');
		strip()?.click();
		strip()?.click();
		expect(strip()?.getAttribute('aria-expanded')).toBe('false');
		expect(localStorage.getItem(taskPanelKey('sA'))).toBeNull();
	});

	it('keeps a very long subject on one truncated line with the full text as a tooltip', () => {
		const long = 'x'.repeat(400);
		render(progressOf([{ content: long, status: 'pending' }]));
		strip()?.click();
		const subject = document.querySelector('.task .subject') as HTMLElement;
		expect(subject.getAttribute('title')).toBe(long);
		expect(document.querySelectorAll('.task')).toHaveLength(1);
	});

	it('shows the in_progress activeForm on the collapsed strip', () => {
		render(sample);
		expect(document.querySelector('.now')?.textContent?.trim()).toBe('Rendering the panel');
	});
});
