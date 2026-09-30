import type { DeviceAuthStatus } from '@bindings/DeviceAuthStatus';

/** Codes are 8 symbols shown as `XXXX-XXXX`. */
export const CODE_LENGTH = 8;

/**
 * What the user typed, as the server stores it. Case and the separator are
 * cosmetic — a code read off a terminal gets retyped with neither.
 */
export function normalizeUserCode(raw: string): string {
	const stripped = raw
		.split('')
		.filter((c) => /[a-zA-Z0-9]/.test(c))
		.join('')
		.toUpperCase()
		.slice(0, CODE_LENGTH);
	return stripped.length === CODE_LENGTH
		? `${stripped.slice(0, 4)}-${stripped.slice(4)}`
		: stripped;
}

export function isCompleteUserCode(raw: string): boolean {
	return normalizeUserCode(raw).length === CODE_LENGTH + 1;
}

/** The code a `verification_uri_complete` link carries, if any. */
export function codeFromSearch(search: string | URLSearchParams): string {
	const params = typeof search === 'string' ? new URLSearchParams(search) : search;
	const raw = params.get('code');
	return raw ? normalizeUserCode(raw) : '';
}

/** Only a pending request is still the user's to decide. */
export function isDecidable(status: DeviceAuthStatus): boolean {
	return status === 'pending';
}

export function statusTone(status: DeviceAuthStatus): 'info' | 'success' | 'danger' {
	if (status === 'approved') return 'success';
	if (status === 'pending') return 'info';
	return 'danger';
}
