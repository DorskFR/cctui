import { describe, expect, it } from 'vitest';
import { PasskeyAborted } from '#lib/passkeys.ts';
import { runPasskeyAttempt } from './login.logic';

const resolves = (ok: boolean) => () => Promise.resolve(ok);
const rejects = (e: unknown) => () => Promise.reject(e);

describe('runPasskeyAttempt', () => {
	it('signs in whoever started the attempt', async () => {
		expect(await runPasskeyAttempt(resolves(true), false)).toBe('signed-in');
		expect(await runPasskeyAttempt(resolves(true), true)).toBe('signed-in');
	});

	it('keeps a failed automatic attempt quiet', async () => {
		expect(await runPasskeyAttempt(resolves(false), false)).toBe('quiet');
		expect(await runPasskeyAttempt(rejects(new Error('boom')), false)).toBe('quiet');
	});

	it('reports a failed attempt the user started', async () => {
		expect(await runPasskeyAttempt(resolves(false), true)).toBe('failed');
		expect(await runPasskeyAttempt(rejects(new Error('boom')), true)).toBe('failed');
	});

	it('never reports a dismissed dialog', async () => {
		expect(await runPasskeyAttempt(rejects(new PasskeyAborted()), true)).toBe('quiet');
	});
});
