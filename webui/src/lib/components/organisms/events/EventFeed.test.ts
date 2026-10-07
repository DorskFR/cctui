import { describe, expect, it } from 'vitest';
import feed from './EventFeed.svelte?raw';
import panel from '../conversation/SessionEventsPanel.svelte?raw';
import pane from '../ConversationPane.svelte?raw';
import machines from '../access/AccessMachinesTab.svelte?raw';
import row from './EventRow.svelte?raw';

describe('the events surfaces stay live and scoped', () => {
	it('the feed prepends socket rows through the filter and the query cache, never a refetch', () => {
		expect(feed).toContain('ws.onEvent(');
		expect(feed).toContain('matchesFilters(ev, filters)');
		expect(feed).toContain('qc.setQueryData<EventPage>(qk.events(query)');
		expect(feed).not.toContain('invalidateQueries');
	});

	it('the session panel only takes rows for its own session, by id or by the detail fallback', () => {
		expect(panel).toContain('sessionIdOf(ev) !== sessionId');
		expect(panel).toContain('qk.sessionEvents(sessionId)');
		expect(pane).toContain('<SessionEventsPanel sessionId={id} />');
	});

	it('the machine history is a modal that mounts its query only when opened', () => {
		expect(machines).toContain('{#if historyTarget}');
		expect(machines).toContain('<MachineHistoryModal');
	});

	it('the row has no :global escape hatch', () => {
		for (const src of [row, feed, panel]) expect(src).not.toContain(':global(');
	});
});
