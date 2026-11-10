// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import SessionControls from './SessionControls.svelte';
import { buildSessionSearchSchema } from '$lib/searchSchema';
import { setCapabilitiesForTest } from '$lib/features.svelte';
import type { Section } from '../../../routes/sessions/sessions.logic';

let component: ReturnType<typeof mount> | undefined;

afterEach(() => {
	if (component) unmount(component);
	component = undefined;
	document.body.replaceChildren();
	setCapabilitiesForTest(null);
});

function render(rawQuery = '') {
	const values = vi.fn(async () => [] as string[]);
	component = mount(SessionControls, {
		target: document.body,
		props: {
			rawQuery,
			searchSchema: buildSessionSearchSchema(values),
			sections: new Set<Section>(),
			labels: [],
			labelFilter: new Set<string>(),
			view: 'list',
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
	return values;
}

describe('SessionControls search box', () => {
	it('is there with transcript search on', () => {
		setCapabilitiesForTest({ features: ['transcript_search'], switchable: ['transcript_search'] });
		render();
		expect(document.querySelector('[data-journey="search"]')).not.toBeNull();
	});

	it('is gone, scope chips included, without transcript search', () => {
		setCapabilitiesForTest({ features: [], switchable: ['transcript_search'] });
		const values = render('machine:box');
		expect(document.querySelector('[data-journey="search"]')).toBeNull();
		expect(document.querySelector('.fi__input')).toBeNull();
		expect(document.querySelector('.scope-row')).toBeNull();
		expect(values).not.toHaveBeenCalled();
	});
});
