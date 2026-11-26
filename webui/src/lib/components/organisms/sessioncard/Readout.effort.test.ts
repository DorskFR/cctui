// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import Readout from './Readout.svelte';
import type { SessionView } from './view';
import headerMeta from '../conversation/HeaderMeta.svelte?raw';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

describe('card readout', () => {
	it('spaces the dot between the model and its effort', () => {
		const view = {
			s: { model: 'claude-opus-4-5', effort: 'medium', token_usage: null, cache_cold: false, adapter_id: 'claude-code' },
			rollup: null,
			draft: true
		} as unknown as SessionView;
		comp = mount(Readout, { target: document.body, props: { view } });
		flushSync();
		expect(document.querySelector('.effort')?.textContent).toBe(' · medium');
	});

	it('keeps the same spacing in the drawer header', () => {
		expect(headerMeta).toMatch(/<span class="m-effort">\{` · \$\{session\.effort\}`\}<\/span>/);
	});
});
