import { describe, expect, it } from 'vitest';
import { ApiError } from '#lib/api.ts';
import { latestSave, serverSave, type SaveState } from './serverSave.svelte.ts';

const st = (status: SaveState['status'], at: number, savedAt: number | null = at): SaveState => ({
	status,
	at,
	savedAt,
	error: null
});

describe('latestSave', () => {
	it('shows whichever source is in flight', () => {
		expect(latestSave(st('saved', 10), st('pending', 5)).status).toBe('pending');
		expect(latestSave(st('pending', 1), st('saved', 50)).status).toBe('pending');
	});

	it('falls back to the other source when one is idle', () => {
		expect(latestSave(st('idle', 0, null), st('saved', 5))).toMatchObject({ status: 'saved', savedAt: 5 });
		expect(latestSave(st('error', 0, null), st('idle', 0, null)).status).toBe('error');
	});

	it('prefers the most recent outcome', () => {
		expect(latestSave(st('saved', 10), st('error', 20, null)).status).toBe('error');
		expect(latestSave(st('saved', 30), st('error', 20, null)).status).toBe('saved');
	});
});

describe('serverSave.run', () => {
	it('goes pending then saved and returns the result', async () => {
		const p = serverSave.run(async () => 42);
		expect(serverSave.status).toBe('pending');
		expect(await p).toBe(42);
		expect(serverSave.status).toBe('saved');
		expect(serverSave.savedAt).not.toBeNull();
	});

	it('records a 4xx message on failure and returns undefined', async () => {
		const out = await serverSave.run(async () => {
			throw new ApiError(422, 'bad name');
		});
		expect(out).toBeUndefined();
		expect(serverSave.status).toBe('error');
		expect(serverSave.error).toBe('bad name');
	});
});
