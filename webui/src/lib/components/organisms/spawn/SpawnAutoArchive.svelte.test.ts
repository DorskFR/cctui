// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, expect, it } from 'vitest';
import { m } from '#lib/paraglide/messages.js';
import Host from './SpawnAutoArchive.host.test.svelte';

let component: ReturnType<typeof mount> | undefined;
afterEach(() => {
	if (component) unmount(component);
	component = undefined;
	document.body.innerHTML = '';
});

function render(checked: boolean) {
	let form!: { auto_archive: boolean };
	component = mount(Host, { target: document.body, props: { checked, onready: (f) => (form = f) } });
	flushSync();
	const box = document.querySelector<HTMLInputElement>('input[type="checkbox"]');
	if (!box) throw new Error('no checkbox rendered');
	return { form: () => form, box };
}

it('starts unticked, toggles the form flag and explains itself', () => {
	const { form, box } = render(false);
	expect(box.checked).toBe(false);
	expect(document.body.textContent).toContain(m.spawn_auto_archive());
	const hint = document.getElementById(box.getAttribute('aria-describedby') ?? '');
	expect(hint?.textContent).toBe(m.spawn_auto_archive_hint());

	box.click();
	flushSync();
	expect(form().auto_archive).toBe(true);
	box.click();
	flushSync();
	expect(form().auto_archive).toBe(false);
});

it('shows a restored flag as ticked', () => {
	const { box } = render(true);
	expect(box.checked).toBe(true);
});
