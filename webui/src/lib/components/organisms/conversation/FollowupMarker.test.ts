// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import FollowupMarker from './FollowupMarker.svelte';
import type { FollowupMark } from './types';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const mark = (reason: FollowupMark['reason']): FollowupMark => ({
	generation: 1,
	from: 's1',
	to: 's2',
	at: '2026-11-21T10:00:00Z',
	reason,
	instruction: 'Carry on.'
});

describe('FollowupMarker', () => {
	it('an auto follow-up reads context limit', () => {
		comp = mount(FollowupMarker, { target: document.body, props: { mark: mark('auto') } });
		flushSync();
		expect(document.querySelector('[data-journey="followup-marker"]')?.textContent).toContain('context limit');
	});
});
