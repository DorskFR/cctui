// @vitest-environment happy-dom
import { flushSync } from 'svelte';
import { beforeEach, describe, expect, it } from 'vitest';
import { showConclusion } from './guideConclusion.svelte';

beforeEach(() => {
	document.body.innerHTML = '';
});

function text() {
	return document.body.textContent ?? '';
}

function button(label: string): HTMLButtonElement {
	const found = [...document.body.querySelectorAll('button')].find((b) =>
		(b.textContent ?? '').includes(label)
	);
	if (!found) throw new Error(`no button labelled ${label} in: ${text()}`);
	return found as HTMLButtonElement;
}

describe('showConclusion', () => {
	it('confirms the tour by name and awards its XP', () => {
		void showConclusion({ title: 'Start a new agent', xp: 25 });
		flushSync();
		expect(text()).toContain('Start a new agent');
		expect(text()).toContain('25');
	});

	it('resolves and tears the card down once the user is finished with it', async () => {
		const closed = showConclusion({ title: 'Start a new agent', xp: 25 });
		flushSync();
		button('Back to guides').click();
		flushSync();
		await expect(closed).resolves.toBeUndefined();
		expect(text()).not.toContain('Start a new agent');
	});
});
