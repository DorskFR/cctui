// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import type { EventPage } from '@bindings/EventPage';
import type { EventRecord } from '@bindings/EventRecord';

type Updater = (old: EventPage | undefined) => EventPage | undefined;
const handlers = vi.hoisted(() => [] as Array<(ev: EventRecord) => void>);
const setQueryData = vi.hoisted(() => vi.fn());
vi.mock('$lib/ws.svelte', () => ({
	ws: {
		onEvent: (fn: (ev: EventRecord) => void) => {
			handlers.push(fn);
			return () => {};
		}
	}
}));
vi.mock('$lib/queries', () => ({
	qk: { sessionEvents: (id: string) => ['events', 'session', id] },
	useSessionEvents: () => ({ data: undefined, isPending: true, isError: false })
}));
vi.mock('@tanstack/svelte-query', () => ({ useQueryClient: () => ({ setQueryData }) }));

import SessionEventsPanel from './SessionEventsPanel.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	handlers.length = 0;
	setQueryData.mockClear();
	document.body.innerHTML = '';
});

const ev = (id: number): EventRecord =>
	({ id, session_id: 's1', summary: `e${id}`, kind: 'session', at: 'now' }) as unknown as EventRecord;

describe('SessionEventsPanel live events', () => {
	it('leaves an empty cache alone so opening the panel still fetches history', () => {
		comp = mount(SessionEventsPanel, { target: document.body, props: { sessionId: 's1' } });
		flushSync();
		expect(handlers).toHaveLength(1);
		handlers[0](ev(7));
		expect(setQueryData).toHaveBeenCalledTimes(1);
		const updater = setQueryData.mock.calls[0][1] as Updater;
		expect(updater(undefined)).toBeUndefined();
	});

	it('merges a live event into a cache that already holds the history', () => {
		comp = mount(SessionEventsPanel, { target: document.body, props: { sessionId: 's1' } });
		flushSync();
		handlers[0](ev(7));
		const updater = setQueryData.mock.calls[0][1] as Updater;
		const page: EventPage = { events: [ev(5)], has_more: true };
		expect(updater(page)?.events.map((e) => e.id)).toEqual([7, 5]);
		expect(updater(page)?.has_more).toBe(true);
	});

	it('ignores events from other sessions', () => {
		comp = mount(SessionEventsPanel, { target: document.body, props: { sessionId: 's1' } });
		flushSync();
		handlers[0]({ ...ev(1), session_id: 's2' } as EventRecord);
		expect(setQueryData).not.toHaveBeenCalled();
	});
});
