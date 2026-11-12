// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import type { Feature } from '@bindings/Feature';
import { setCapabilitiesForTest } from '$lib/features.svelte';
import type { Dimension, Section } from '../../../routes/sessions/sessions.logic';
import SectionFilter from './SectionFilter.svelte';
import DimensionPicker from './DimensionPicker.svelte';

const SWITCHABLE: Feature[] = ['organising', 'drafts', 'rooms'];
let component: ReturnType<typeof mount> | undefined;

afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
	setCapabilitiesForTest(null);
});

const on = (...features: Feature[]) => setCapabilitiesForTest({ features, switchable: SWITCHABLE });

async function openFilter(sections: Section[]) {
	component = mount(SectionFilter, { target: document.body, props: { sections: new Set(sections) } });
	flushSync();
	const panel = document.querySelector<HTMLElement>('[popover]');
	if (!panel) throw new Error('panel did not render');
	panel.dispatchEvent(Object.assign(new Event('toggle'), { newState: 'open' }));
	await tick();
	await tick();
	flushSync();
	return [...document.querySelectorAll('[role="menuitemcheckbox"]')].map((el) => el.getAttribute('data-journey-key'));
}

function pickerValues(kind: 'color' | 'group', value: Dimension) {
	component = mount(DimensionPicker, { target: document.body, props: { kind, value, onchange: () => {} } });
	flushSync();
	return [...document.querySelectorAll('option')].map((o) => o.value);
}

describe('SectionFilter by feature', () => {
	it('offers starred and drafts with organising and drafts on', async () => {
		on(...SWITCHABLE);
		const keys = await openFilter(['starred', 'live']);
		expect(keys).toContain('starred');
		expect(keys).toContain('drafts');
	});

	it('offers no starred toggle without organising and no drafts toggle without drafts', async () => {
		on('rooms');
		const keys = await openFilter(['starred', 'live']);
		expect(keys).not.toContain('starred');
		expect(keys).not.toContain('drafts');
		expect(keys).toContain('live');
	});

	it('counts only the toggles it offers', async () => {
		on('rooms');
		await openFilter(['starred', 'live', 'dispatched', 'archived', 'unread']);
		expect(document.querySelector('.count')).toBeNull();
	});
});

describe('DimensionPicker by feature', () => {
	it('offers label and room with organising and rooms on', () => {
		on(...SWITCHABLE);
		expect(pickerValues('group', 'status')).toEqual(expect.arrayContaining(['label', 'room']));
	});

	it('offers no label without organising', () => {
		on('rooms', 'drafts');
		expect(pickerValues('color', 'none')).not.toContain('label');
	});

	it('offers no room without rooms', () => {
		on('organising', 'drafts');
		expect(pickerValues('group', 'status')).not.toContain('room');
	});
});
