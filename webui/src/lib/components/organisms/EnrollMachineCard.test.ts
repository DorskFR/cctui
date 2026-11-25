// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('#lib/features.svelte.ts', () => ({ hasFeature: () => true }));

import EnrollMachineCard from './EnrollMachineCard.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

describe('EnrollMachineCard', () => {
	it('shows the whole enroll command, untruncated, on a plain surface', () => {
		comp = mount(EnrollMachineCard, { target: document.body });
		flushSync();
		const cmd = document.body.querySelector('[data-journey="enroll-cmd"]');
		expect(cmd?.textContent).toContain('cctui-daemon enroll --server-url');
		expect(cmd?.textContent).toContain('--name "$(hostname)"');
		expect(cmd?.querySelector('.truncate')).toBeNull();
		expect(cmd?.querySelector('[data-journey="enroll-copy"]')).not.toBeNull();
		expect(document.body.innerHTML).not.toContain('dashed');
	});
});
