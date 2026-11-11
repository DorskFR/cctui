// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import SessionControls from './SessionControls.svelte';
import { buildSessionSearchSchema } from '$lib/searchSchema';
import { setCapabilitiesForTest } from '$lib/features.svelte';
import { settings } from '$lib/settings.svelte';
import type { Label } from '@bindings/Label';
import type { Section } from '../../../routes/sessions/sessions.logic';

vi.mock('$lib/queries', async (original) => ({
	...(await original<typeof import('$lib/queries')>()),
	useSessionActions: () => ({})
}));

let component: ReturnType<typeof mount> | undefined;

afterEach(() => {
	if (component) unmount(component);
	component = undefined;
	document.body.replaceChildren();
	setCapabilitiesForTest(null);
	settings.setMacrosEnabled(false);
});

function render(rawQuery = '', labels: Label[] = []) {
	const values = vi.fn(async () => [] as string[]);
	component = mount(SessionControls, {
		target: document.body,
		props: {
			rawQuery,
			searchSchema: buildSessionSearchSchema(values),
			sections: new Set<Section>(),
			labels,
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
		expect(document.querySelector('.bar')?.classList.contains('no-search')).toBe(false);
	});

	it('is gone, scope chips included, without transcript search', () => {
		setCapabilitiesForTest({ features: [], switchable: ['transcript_search'] });
		const values = render('machine:box');
		expect(document.querySelector('[data-journey="search"]')).toBeNull();
		expect(document.querySelector('.fi__input')).toBeNull();
		expect(document.querySelector('.scope-row')).toBeNull();
		expect(values).not.toHaveBeenCalled();
	});

	it('keeps the section filter, held to the right edge', () => {
		setCapabilitiesForTest({ features: [], switchable: ['transcript_search'] });
		render();
		expect(document.querySelector('.bar')?.classList.contains('no-search')).toBe(true);
		expect(document.querySelector('.bar > .lead [data-journey="sections"]')).not.toBeNull();
	});
});

describe('SessionControls label filter', () => {
	const ops = { id: 'l1', name: 'ops', color: '#888888' } as Label;

	it('is there with organising on', () => {
		setCapabilitiesForTest({ features: ['organising'], switchable: ['organising'] });
		render('', [ops]);
		expect(document.querySelector('.label-filter')).not.toBeNull();
	});

	it('is gone without organising', () => {
		setCapabilitiesForTest({ features: [], switchable: ['organising'] });
		render('', [ops]);
		expect(document.querySelector('.label-filter')).toBeNull();
	});
});

describe('SessionControls macros menu', () => {
	const macros = () => document.body.textContent?.includes('⚡') ?? false;

	it('is there when turned on with machine_spawn on', () => {
		setCapabilitiesForTest({ features: ['machine_spawn'], switchable: ['machine_spawn'] });
		settings.setMacrosEnabled(true);
		render();
		expect(macros()).toBe(true);
	});

	it('is gone without machine_spawn, even when turned on', () => {
		setCapabilitiesForTest({ features: [], switchable: ['machine_spawn'] });
		settings.setMacrosEnabled(true);
		render();
		expect(macros()).toBe(false);
	});
});
