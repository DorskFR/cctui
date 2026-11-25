import { PasskeyAborted } from '#lib/passkeys.ts';

export type PasskeyOutcome = 'signed-in' | 'quiet' | 'failed';

/** A failed automatic attempt stays quiet: only a click earns an error. A
 *  dismissed dialog is a choice, never an error. */
export async function runPasskeyAttempt(
	login: () => Promise<boolean>,
	userInitiated: boolean
): Promise<PasskeyOutcome> {
	try {
		if (await login()) return 'signed-in';
	} catch (e) {
		if (e instanceof PasskeyAborted) return 'quiet';
	}
	return userInitiated ? 'failed' : 'quiet';
}
