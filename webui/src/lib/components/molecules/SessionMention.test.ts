import { afterEach, describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import { tick } from 'svelte';
import type { SessionListItem } from '@bindings/SessionListItem';
import Host from './SessionMention.host.test.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const sess = (p: Partial<SessionListItem>): SessionListItem =>
	({ id: 'id', status: 'active', bucket: 'working', working_dir: '/w', name: null, ...p }) as SessionListItem;

function render(sessions: SessionListItem[] = [sess({ id: 'aaa', name: 'Green' })]) {
	comp = mount(Host, { target: document.body, props: { sessions } });
	const ta = document.querySelector('textarea') as HTMLTextAreaElement;
	return { ta };
}
const panel = () => document.querySelector('.mention-panel');
async function type(ta: HTMLTextAreaElement, text: string) {
	ta.focus();
	ta.value = text;
	ta.setSelectionRange(text.length, text.length);
	ta.dispatchEvent(new Event('input', { bubbles: true }));
	flushSync();
	await tick();
}
const key = (ta: HTMLTextAreaElement, k: string, type: 'keydown' | 'keyup' = 'keydown') => {
	ta.dispatchEvent(new KeyboardEvent(type, { key: k, bubbles: true, cancelable: true }));
	flushSync();
};

describe('SessionMention', () => {
	it('opens on # and closes once whitespace follows', async () => {
		const { ta } = render();
		await type(ta, 'sync #');
		expect(panel()).not.toBeNull();
		await type(ta, 'sync # ');
		expect(panel()).toBeNull();
	});
	it('never opens when there is no session to offer', async () => {
		const { ta } = render([]);
		await type(ta, '#');
		expect(panel()).toBeNull();
	});
	it('shows the empty row on a miss, and Escape dismisses until the caret leaves the #', async () => {
		const { ta } = render();
		await type(ta, '#zzz');
		expect(panel()?.textContent).toContain('No running session');
		key(ta, 'Escape');
		await tick();
		expect(panel()).toBeNull();
		// Stays closed while typing within the same trigger …
		await type(ta, '#zzzq');
		expect(panel()).toBeNull();
		// … and reopens when the caret comes back to a # after leaving it.
		await type(ta, '#zzzq ');
		await type(ta, '#zzzq #');
		expect(panel()).not.toBeNull();
	});
	it('closes when focus leaves the field and reopens on focus at a #', async () => {
		const { ta } = render();
		await type(ta, '#');
		expect(panel()).not.toBeNull();
		ta.dispatchEvent(new FocusEvent('focusout', { bubbles: true, relatedTarget: null }));
		flushSync();
		expect(panel()).toBeNull();
		ta.dispatchEvent(new FocusEvent('focusin', { bubbles: true }));
		flushSync();
		expect(panel()).not.toBeNull();
	});
	it('Enter picks and inserts the token', async () => {
		const { ta } = render();
		await type(ta, 'go #gr');
		key(ta, 'Enter');
		await tick();
		await tick();
		expect(ta.value).toBe('go #aaa (Green) ');
		expect(panel()).toBeNull();
	});
});
