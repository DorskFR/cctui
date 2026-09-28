import type { LimitResetStatus } from '$lib/queries';
import { getLocale } from '$lib/paraglide/runtime';
import { m } from '$lib/paraglide/messages';

/** Button label: the Codex credit's title or the Claude grant's label when there
 *  is one, else the generic verb. */
export function limitResetLabel(s: LimitResetStatus): string {
	return s.title ? m.sessions_limit_reset_credit({ title: s.title }) : m.sessions_limit_reset();
}

/** A `cedar_ember` grant rather than a Codex credit or the at-wall program: it
 *  has an expiry and its own claim rules. */
function isGrant(s: LimitResetStatus): boolean {
	return s.requires_limit !== undefined || (s.clears?.length ?? 0) > 0;
}

function date(iso: string): string {
	return new Date(iso).toLocaleDateString(getLocale());
}

/** Why the button is disabled: the upstream reason, then a grant's expiry (or the
 *  next window), else a generic line. Empty when the reset is claimable. */
export function limitResetHint(s: LimitResetStatus): string {
	if (s.available) return '';
	const lines: string[] = [];
	if (s.ineligible_reason) lines.push(m.sessions_limit_reset_reason({ reason: s.ineligible_reason }));
	if (isGrant(s)) {
		if (s.requires_limit) lines.push(m.sessions_limit_reset_requires_limit());
		if (s.next_available_at) lines.push(m.sessions_limit_reset_expires({ date: date(s.next_available_at) }));
	} else if (s.next_available_at) {
		lines.push(m.sessions_limit_reset_next({ time: new Date(s.next_available_at).toLocaleString(getLocale()) }));
	}
	if (lines.length === 0) lines.push(m.sessions_limit_reset_unavailable());
	return lines.join('\n');
}

function windowLabel(key: string): string {
	if (key === 'five_hour') return m.sessions_limit_reset_window_5h();
	if (key.startsWith('seven_day')) return m.sessions_limit_reset_window_weekly();
	return key;
}

/** What a claim refills, for the confirm dialog. Empty when upstream did not say. */
export function limitResetClears(s: LimitResetStatus): string {
	const windows = [...new Set((s.clears ?? []).map(windowLabel))];
	return windows.length === 0 ? '' : m.sessions_limit_reset_clears({ windows: windows.join(' + ') });
}
