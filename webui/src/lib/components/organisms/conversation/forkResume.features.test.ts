// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import { m } from '$lib/paraglide/messages';
import ArchivedActions from './ArchivedActions.svelte';
import pane from '../ConversationPane.svelte?raw';
import header from './DrawerHeader.svelte?raw';
import meta from './HeaderMeta.svelte?raw';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

function labels(props: { onFork?: () => void; onResume?: () => void }) {
	comp = mount(ArchivedActions, { target: document.body, props: { onNewFromScript: vi.fn(), ...props } });
	return [...document.querySelectorAll('button')].map((b) => b.textContent?.trim());
}

describe('archived session actions', () => {
	it('offers fork and resume when the pane hands them over', () => {
		expect(labels({ onFork: vi.fn(), onResume: vi.fn() })).toEqual([
			m.composer_new_from_script(),
			m.composer_fork(),
			m.composer_resume()
		]);
	});

	it('renders neither without their handlers', () => {
		expect(labels({})).toEqual([m.composer_new_from_script()]);
	});
});

describe('fork and resume follow machine spawn', () => {
	it('gates every fork and resume entry point of the pane on the feature', () => {
		expect(pane).toContain("const canFork = $derived(hasFeature('machine_spawn'));");
		expect(pane).toMatch(/const forkable = \$derived\(canFork && /);
		expect(pane).toContain('onfork={canFork ? fork.openDialog : undefined}');
		expect(pane).toContain('onFork={canFork ? fork.openDialog : undefined}');
		expect(pane).toContain('onResume={canFork ? sa.resume : undefined}');
		expect(pane).toContain('{#if forkSelect.active && canFork}');
		expect(pane).toContain('{#if fork.open && canFork}');
	});

	it('drops the header fork entry and the fork-to-change-model chip without a handler', () => {
		expect(header).toContain('const forkAction = $derived(onforkselect ?? onfork);');
		expect(header).toMatch(/\.\.\.\(forkAction\s*\?\s*\[\s*\{\s*label: m\.drawer_fork_label\(\)/);
		expect(meta).toContain('onfork?: () => void;');
		expect(meta).toContain('&& !onfork}');
	});
});
