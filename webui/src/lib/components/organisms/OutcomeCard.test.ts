// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import type { SessionOutcome } from '@bindings/SessionOutcome';
import OutcomeCard from './OutcomeCard.svelte';

const flush = () => new Promise((r) => setTimeout(r, 0));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const outcome = (over: Partial<SessionOutcome> = {}): SessionOutcome => ({
	id: 'o-1',
	session_id: 's-1',
	status: 'result',
	headline: 'Opened PR 12',
	body: 'All checks pass.',
	evidence: ['https://example.test/pr/12', '/tmp/test.log'],
	source: 'tool',
	created_at: '2026-11-16T10:00:00Z',
	...over
});

async function render(o: SessionOutcome | null) {
	comp = mount(OutcomeCard, { target: document.body, props: { outcome: o } });
	await flush();
}

describe('OutcomeCard', () => {
	it('shows the headline, body and evidence of a declared outcome', async () => {
		await render(outcome());
		const text = document.body.textContent ?? '';
		expect(text).toContain('Opened PR 12');
		expect(text).toContain('All checks pass.');
		expect(document.querySelector('a')?.getAttribute('href')).toBe('https://example.test/pr/12');
		expect(document.querySelector('code')?.textContent).toBe('/tmp/test.log');
	});

	it('renders nothing for an inferred outcome or none', async () => {
		await render(outcome({ status: 'ended_without_outcome', source: 'inferred' }));
		expect(document.body.textContent?.trim()).toBe('');
		if (comp) unmount(comp);
		comp = null;
		await render(null);
		expect(document.body.textContent?.trim()).toBe('');
	});
});
