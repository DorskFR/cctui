import { afterEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import ActivityBanner from './ActivityBanner.svelte';
import type { ConversationStream } from './stream.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const stream = (over: Partial<ConversationStream> = {}) =>
	({
		working: false,
		ask: null,
		plan: null,
		perms: [],
		todos: null,
		todoProgress: null,
		currentTool: null,
		turnStartedAt: null,
		turnTokensIn: 0,
		turnTokensOut: 0,
		lastAssistantLine: null,
		...over
	}) as unknown as ConversationStream;

const render = (s: ConversationStream, archived = false) => {
	comp = mount(ActivityBanner, { target: document.body, props: { stream: s, archived } });
	return document.body;
};

const text = () => document.body.textContent ?? '';

describe('ActivityBanner', () => {
	it('collapses to a thin idle state rather than a stale last activity', () => {
		render(stream({ lastAssistantLine: 'Committing the bootstrap.' }));
		const el = document.querySelector('.activity');
		expect(el).not.toBeNull();
		expect(el?.classList.contains('idle')).toBe(true);
		// The stale line must not leak into the idle banner.
		expect(text()).not.toContain('Committing the bootstrap.');
		expect(document.querySelector('.sub')).toBeNull();
	});

	it('renders nothing at all for an archived session', () => {
		render(stream({ working: true }), true);
		expect(document.querySelector('.activity')).toBeNull();
	});

	it('stays idle while the agent is blocked on the user', () => {
		const blocked = [
			{ ask: { question: 'which?' } },
			{ plan: { plan: '# plan' } },
			{ perms: [{ request_id: 'r1' }] }
		];
		for (const b of blocked) {
			render(stream({ working: true, lastAssistantLine: 'thinking', ...b }));
			expect(document.querySelector('.activity')?.classList.contains('idle')).toBe(true);
			unmount(comp!);
			comp = null;
			document.body.innerHTML = '';
		}
	});

	it('prefers the in_progress task activeForm as the status line', () => {
		render(
			stream({
				working: true,
				turnStartedAt: Date.now(),
				lastAssistantLine: 'some older prose',
				todoProgress: {
					items: [{ content: 'b', status: 'in_progress', activeForm: 'Wiring the parser' }],
					done: 2,
					total: 5,
					inProgress: { content: 'b', status: 'in_progress', activeForm: 'Wiring the parser' }
				}
			})
		);
		expect(text()).toContain('Wiring the parser');
		expect(text()).toContain('2/5');
	});

	it('falls back to the last assistant line when no tool is running', () => {
		render(stream({ working: true, turnStartedAt: Date.now(), lastAssistantLine: 'Test passes.' }));
		expect(text()).toContain('Test passes.');
	});

	it('renders a long tool invocation truncated on one line, with the full text only as a tooltip', () => {
		const summary = `$ ${'x'.repeat(200)}`.slice(0, 139) + '…';
		render(
			stream({
				working: true,
				turnStartedAt: Date.now(),
				currentTool: { tool: 'Bash', summary, startedAt: Date.now() }
			})
		);
		const sub = document.querySelector('.sub .status') as HTMLElement;
		expect(sub).not.toBeNull();
		expect(sub.textContent?.trim().length).toBeLessThanOrEqual(140);
		expect(sub.getAttribute('title')).toBe(summary);
		// The banner never exceeds its two rows, however long the invocation.
		expect(document.querySelectorAll('.activity .row')).toHaveLength(2);
	});

	it('shows only one row when a tool has no usable summary', () => {
		render(
			stream({
				working: true,
				turnStartedAt: Date.now(),
				currentTool: { tool: 'Bash', summary: '', startedAt: Date.now() }
			})
		);
		expect(document.querySelectorAll('.activity .row')).toHaveLength(1);
		expect(text()).toContain('Bash');
	});

	it('shows per-turn token counters only once tokens have accrued', () => {
		render(stream({ working: true, turnStartedAt: Date.now(), lastAssistantLine: 'x' }));
		expect(text()).not.toContain('↓');
		unmount(comp!);
		comp = null;
		document.body.innerHTML = '';

		render(
			stream({ working: true, turnStartedAt: Date.now(), turnTokensIn: 120, turnTokensOut: 34, lastAssistantLine: 'x' })
		);
		expect(text()).toContain('↓120');
		expect(text()).toContain('↑34');
	});
});
