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

/**
 * The facts about the requester the approver can actually weigh, as opposed to
 * `client_name`, which the unauthenticated caller chose. Omits whatever the
 * deployment did not record rather than showing an empty row.
 */
export function requesterFacts(
	info: { client_ip: string | null; user_agent: string | null; created_at: string },
	nowMs: number
): { label: string; value: string }[] {
	const facts: { label: string; value: string }[] = [];
	if (info.client_ip) facts.push({ label: 'from', value: info.client_ip });
	if (info.user_agent) facts.push({ label: 'client', value: info.user_agent });
	const startedMs = Date.parse(info.created_at);
	if (!Number.isNaN(startedMs)) {
		const secs = Math.max(0, Math.round((nowMs - startedMs) / 1000));
		facts.push({
			label: 'started',
			value: secs < 60 ? `${secs}s ago` : `${Math.floor(secs / 60)}m ${secs % 60}s ago`
		});
	}
	return facts;
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
