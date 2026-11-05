import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { confirmDialog, confirmQueue } from './confirmDialog.svelte';

afterEach(() => confirmQueue.clear());

describe('confirmDialog', () => {
	it('queues a request with the danger tone and the default cancel label', () => {
		void confirmDialog({ title: 'Delete bookmark?', confirmLabel: 'Delete bookmark' });
		expect(confirmQueue.current).toMatchObject({
			title: 'Delete bookmark?',
			confirmLabel: 'Delete bookmark',
			cancelLabel: 'Cancel',
			tone: 'danger'
		});
	});

	it('keeps the caller tone, message and cancel label', () => {
		void confirmDialog({ title: 'Dismiss all?', message: 'Three failures', confirmLabel: 'Dismiss all', cancelLabel: 'Keep', tone: 'warn' });
		expect(confirmQueue.current).toMatchObject({ message: 'Three failures', cancelLabel: 'Keep', tone: 'warn' });
	});

	it('resolves true on confirm and false on cancel', async () => {
		const first = confirmDialog({ title: 'A', confirmLabel: 'Do A' });
		const second = confirmDialog({ title: 'B', confirmLabel: 'Do B' });
		expect(confirmQueue.size).toBe(2);

		confirmQueue.settle(confirmQueue.current?.id ?? -1, true);
		await expect(first).resolves.toBe(true);
		expect(confirmQueue.current?.title).toBe('B');

		confirmQueue.settle(confirmQueue.current?.id ?? -1, false);
		await expect(second).resolves.toBe(false);
		expect(confirmQueue.current).toBeNull();
	});

	it('ignores a second settle of the same request', async () => {
		const p = confirmDialog({ title: 'A', confirmLabel: 'Do A' });
		const id = confirmQueue.current?.id ?? -1;
		confirmQueue.settle(id, true);
		confirmQueue.settle(id, false);
		await expect(p).resolves.toBe(true);
	});

	it('cancels everything pending on clear', async () => {
		const a = confirmDialog({ title: 'A', confirmLabel: 'Do A' });
		const b = confirmDialog({ title: 'B', confirmLabel: 'Do B' });
		confirmQueue.clear();
		await expect(Promise.all([a, b])).resolves.toEqual([false, false]);
	});
});

describe('native dialogs', () => {
	// Biome lints only .ts here, so the .svelte scripts are scanned directly.
	const sources = (dir: string, out: string[] = []): string[] => {
		for (const name of readdirSync(dir)) {
			const p = join(dir, name);
			if (statSync(p).isDirectory()) {
				if (name !== 'paraglide') sources(p, out);
			} else if (/\.(svelte|ts)$/.test(name) && !name.endsWith('.test.ts')) out.push(p);
		}
		return out;
	};

	it('no source calls confirm(), alert() or prompt()', () => {
		const hits: string[] = [];
		for (const f of sources('src')) {
			const code = readFileSync(f, 'utf8')
				.replace(/\/\*[\s\S]*?\*\//g, '')
				.replace(/(^|[^:])\/\/.*$/gm, '$1');
			for (const name of ['confirm', 'alert', 'prompt']) {
				if (new RegExp(`function\\s+${name}\\s*\\(|(const|let)\\s+${name}\\b`).test(code)) continue;
				if (new RegExp(`(^|[^.\\w$#])(window\\.)?${name}\\(`, 'm').test(code)) hits.push(`${f}: ${name}()`);
			}
		}
		expect(hits, 'use confirmDialog() or a ConfirmModal').toEqual([]);
	});
});
