import { describe, expect, it, vi } from 'vitest';
import { ArchiveConfirm } from './archiveConfirm.svelte';

const pending = (ids: string[], onDone?: () => void) => ({
	title: 'Archive all in Completed',
	message: 'Archive all 2 sessions in Completed?',
	ids,
	onDone
});

describe('ArchiveConfirm', () => {
	it('confirming archives exactly the requested ids, then runs onDone and clears', async () => {
		const archive = vi.fn(async () => {});
		const onDone = vi.fn();
		const ac = new ArchiveConfirm(archive, () => {});
		ac.request(pending(['a', 'b'], onDone));
		expect(ac.pending?.ids).toEqual(['a', 'b']);
		await ac.confirm();
		expect(archive).toHaveBeenCalledTimes(1);
		expect(archive).toHaveBeenCalledWith(['a', 'b']);
		expect(onDone).toHaveBeenCalledTimes(1);
		expect(ac.pending).toBeNull();
		expect(ac.busy).toBe(false);
	});

	it('cancelling calls nothing', async () => {
		const archive = vi.fn(async () => {});
		const ac = new ArchiveConfirm(archive, () => {});
		ac.request(pending(['a']));
		ac.cancel();
		expect(ac.pending).toBeNull();
		await ac.confirm();
		expect(archive).not.toHaveBeenCalled();
	});

	it('ignores an empty section', () => {
		const ac = new ArchiveConfirm(async () => {}, () => {});
		ac.request(pending([]));
		expect(ac.pending).toBeNull();
	});

	it('reports a failed archive and still clears the dialog', async () => {
		const err = new Error('boom');
		const onerror = vi.fn();
		const ac = new ArchiveConfirm(async () => {
			throw err;
		}, onerror);
		ac.request(pending(['a']));
		await ac.confirm();
		expect(onerror).toHaveBeenCalledWith(err);
		expect(ac.pending).toBeNull();
		expect(ac.busy).toBe(false);
	});
});
