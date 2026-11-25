// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync } from 'svelte';

const media = vi.hoisted(() => {
	const state = { short: false, queries: [] as string[] };
	window.matchMedia = ((query: string) => {
		state.queries.push(query);
		return {
			media: query,
			get matches() {
				return state.short;
			},
			addEventListener() {},
			removeEventListener() {}
		};
	}) as unknown as typeof window.matchMedia;
	return state;
});
import { ComposerFocus } from './composerFocus.svelte';
import header from './DrawerHeader.svelte?raw';
import toolbar from './DrawerToolbar.svelte?raw';

afterEach(() => {
	document.body.innerHTML = '';
});

function pane(id: string) {
	const root = document.createElement('div');
	root.dataset.journey = 'conversation';
	root.innerHTML = `<div class="head" id="${id}-head"><input id="${id}-rename"></div>
		<div data-journey="composer"><textarea id="${id}-msg"></textarea><button id="${id}-send">send</button></div>`;
	document.body.append(root);
	return root;
}

describe('ComposerFocus', () => {
	it('follows focus in and out of this pane composer', () => {
		pane('a');
		const f = new ComposerFocus();
		const stop = f.attach(document.getElementById('a-head') as HTMLElement);
		(document.getElementById('a-msg') as HTMLTextAreaElement).focus();
		flushSync();
		expect(f.focused).toBe(true);
		(document.getElementById('a-send') as HTMLButtonElement).focus();
		flushSync();
		expect(f.focused).toBe(true);
		(document.getElementById('a-rename') as HTMLInputElement).focus();
		flushSync();
		expect(f.focused).toBe(false);
		stop();
	});

	it('ignores the composer of another pane', () => {
		pane('a');
		pane('b');
		const f = new ComposerFocus();
		const stop = f.attach(document.getElementById('a-head') as HTMLElement);
		(document.getElementById('b-msg') as HTMLTextAreaElement).focus();
		flushSync();
		expect(f.focused).toBe(false);
		stop();
	});

	it('collapses only while focused on a short, narrow viewport', () => {
		expect(media.queries).toContain('(max-width: 959px) and (max-height: 640px)');
		const f = new ComposerFocus();
		f.focused = true;
		media.short = false;
		expect(f.collapse).toBe(false);
		media.short = true;
		expect(f.collapse).toBe(true);
		f.focused = false;
		expect(f.collapse).toBe(false);
		media.short = false;
	});
});

describe('chrome folded while typing', () => {
	it('hides the header, never in a tile, and the toolbar', () => {
		expect(header).toContain('class:typing={!compact && composer.collapse}');
		expect(header).toMatch(/\.dhead\.typing \{\s*display: none;/);
		expect(toolbar).toContain('class:typing={composer.collapse}');
		expect(toolbar).toMatch(/\.tbhost\.typing \{\s*display: none;/);
	});
});
