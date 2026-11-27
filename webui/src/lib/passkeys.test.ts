// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { ApiError } from './api';
import { needsSignInAgain, passkeyDismissed } from './passkeys';

describe('passkeyDismissed', () => {
	it('treats a cancelled, timed-out or aborted dialog as a choice', () => {
		expect(passkeyDismissed(new DOMException('no', 'NotAllowedError'))).toBe(true);
		expect(passkeyDismissed(new DOMException('no', 'AbortError'))).toBe(true);
	});

	it('reports everything else', () => {
		expect(passkeyDismissed(new DOMException('dup', 'InvalidStateError'))).toBe(false);
		expect(passkeyDismissed(new Error('boom'))).toBe(false);
	});
});

describe('needsSignInAgain', () => {
	it('is the refusal of a stale or narrower sign-in, never a logout', () => {
		expect(needsSignInAgain(new ApiError(403, 'sign in again'))).toBe(true);
		expect(needsSignInAgain(new ApiError(401, 'Unauthorized'))).toBe(false);
		expect(needsSignInAgain(new ApiError(400, 'challenge expired'))).toBe(false);
		expect(needsSignInAgain(new DOMException('no', 'NotAllowedError'))).toBe(false);
	});
});
