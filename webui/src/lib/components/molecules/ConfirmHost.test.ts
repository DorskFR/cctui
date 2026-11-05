// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import { confirmDialog, confirmQueue } from '$lib/confirmDialog.svelte';
import ConfirmHost from './ConfirmHost.svelte';

let target: HTMLElement | null = null;
let app: Record<string, unknown> | null = null;

afterEach(() => {
	if (app) unmount(app);
	target?.remove();
	app = null;
	target = null;
	confirmQueue.clear();
});

const host = () => {
	target = document.createElement('div');
	document.body.appendChild(target);
	app = mount(ConfirmHost, { target });
	flushSync();
	return target;
};

const button = (root: HTMLElement, text: string) =>
	[...root.querySelectorAll('button')].find((b) => b.textContent?.trim() === text);

describe('ConfirmHost', () => {
	it('renders nothing until a confirmation is requested', () => {
		expect(host().querySelector('dialog')).toBeNull();
	});

	it('shows the request and resolves true on the confirm button', async () => {
		const root = host();
		const answer = confirmDialog({ title: 'Delete bookmark?', message: 'It cannot be restored.', confirmLabel: 'Delete bookmark' });
		flushSync();
		expect(root.textContent).toContain('Delete bookmark?');
		expect(root.textContent).toContain('It cannot be restored.');
		button(root, 'Delete bookmark')?.click();
		flushSync();
		await expect(answer).resolves.toBe(true);
	});

	it('resolves false on cancel', async () => {
		const root = host();
		const answer = confirmDialog({ title: 'Remove provider?', confirmLabel: 'Remove provider' });
		flushSync();
		button(root, 'Cancel')?.click();
		flushSync();
		await expect(answer).resolves.toBe(false);
	});

	it('styles the confirm button as danger by default', () => {
		const root = host();
		void confirmDialog({ title: 'Uninstall?', confirmLabel: 'Uninstall plugin' });
		flushSync();
		expect(button(root, 'Uninstall plugin')?.className).toMatch(/danger/);
	});

	it('cancels pending requests when the host unmounts', async () => {
		host();
		const answer = confirmDialog({ title: 'A', confirmLabel: 'Do A' });
		flushSync();
		if (app) unmount(app);
		app = null;
		await expect(answer).resolves.toBe(false);
	});
});
