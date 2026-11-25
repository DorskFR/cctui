// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { flushSync } from 'svelte';
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

	it('only collapses on a short viewport', () => {
		expect(new ComposerFocus().collapse).toBe(false);
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
