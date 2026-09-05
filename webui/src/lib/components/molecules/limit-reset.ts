import type { LimitResetStatus } from '$lib/queries';
import { getLocale } from '$lib/paraglide/runtime';
import { m } from '$lib/paraglide/messages';

/** Button label: the Codex credit's title when there is one, else the generic verb. */
export function limitResetLabel(s: LimitResetStatus): string {
	return s.title ? m.sessions_limit_reset_credit({ title: s.title }) : m.sessions_limit_reset();
}

/** Why the button is disabled: the upstream reason, then the next window, else
 *  a generic line. Empty when the reset is claimable. */
export function limitResetHint(s: LimitResetStatus): string {
	if (s.available) return '';
	const lines: string[] = [];
	if (s.ineligible_reason) lines.push(m.sessions_limit_reset_reason({ reason: s.ineligible_reason }));
	if (s.next_available_at) lines.push(m.sessions_limit_reset_next({ time: new Date(s.next_available_at).toLocaleString(getLocale()) }));
	if (lines.length === 0) lines.push(m.sessions_limit_reset_unavailable());
	return lines.join('\n');
}
