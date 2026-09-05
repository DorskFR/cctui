import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import SessionControls from './SessionControls.svelte';
import { buildSessionSearchSchema } from '$lib/searchSchema';
import type { Section } from '../../../routes/sessions/sessions.logic';

let component: ReturnType<typeof mount> | undefined;

afterEach(() => {
	if (component) unmount(component);
	component = undefined;
	document.body.replaceChildren();
});

describe('SessionControls', () => {
	it('names the search input', () => {
		component = mount(SessionControls, {
			target: document.body,
			props: {
				rawQuery: '',
				searchSchema: buildSessionSearchSchema(async () => []),
				sections: new Set<Section>(),
				labels: [],
				labelFilter: new Set<string>(),
				cardView: false,
				colorBy: 'none',
				groupBy: 'status',
				onColorBy: () => {},
				onGroupBy: () => {},
				selecting: false,
				searching: false,
				onStartSelect: () => {},
				onCancelSelect: () => {}
			}
		});
		flushSync();

		const input = document.querySelector<HTMLInputElement>('.fi__input');
		expect(input).not.toBeNull();
		const label = document.querySelector(`label[for="${CSS.escape(input?.id ?? '')}"]`);
		expect(label?.textContent?.trim()).toBe('Search sessions');
	});
});
