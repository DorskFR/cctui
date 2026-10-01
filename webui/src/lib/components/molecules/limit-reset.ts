import { formatTimestamp } from '@dorsk/tsumikit';
import type { LimitResetEntry, LimitResetStatus } from '$lib/queries';
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
	return formatTimestamp(iso, 'date');
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
		lines.push(m.sessions_limit_reset_next({ time: formatTimestamp(s.next_available_at, 'datetime') }));
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

/** An entry's own name: upstream's title, else the at-wall program's, else the
 *  generic verb. */
export function resetTitle(e: LimitResetEntry): string {
	if (e.title) return e.title;
	return e.id === 'juniper_tide' ? m.limit_resets_at_wall() : m.sessions_limit_reset();
}

/** The windows an entry refills. Empty when upstream named none — which a row
 *  must render as nothing rather than as "restores nothing". */
export function resetRestores(e: LimitResetEntry): string {
	const windows = [...new Set(e.restores.map(windowLabel))];
	return windows.join(' + ');
}

export function resetExpiry(e: LimitResetEntry): string {
	return e.expires_at ? m.sessions_limit_reset_expires({ date: date(e.expires_at) }) : '';
}

/** Why a row's button is disabled. Empty when the entry is usable. */
export function resetReason(e: LimitResetEntry): string {
	if (e.usable) return '';
	const lines: string[] = [];
	if (e.unusable_reason) lines.push(m.sessions_limit_reset_reason({ reason: e.unusable_reason }));
	if (e.requires_limit) lines.push(m.sessions_limit_reset_requires_limit());
	if (lines.length === 0) lines.push(m.sessions_limit_reset_unavailable());
	return lines.join('\n');
}

/** The entry whose expiry comes first, which is the one the card names. Entries
 *  with no expiry lose to any that has one. */
export function nextResetToExpire(entries: LimitResetEntry[]): LimitResetEntry | null {
	const dated = entries.filter((e) => e.expires_at);
	if (dated.length > 0) {
		return dated.reduce((a, b) =>
			Date.parse(a.expires_at as string) <= Date.parse(b.expires_at as string) ? a : b
		);
	}
	return entries[0] ?? null;
}

/** The card icon's tooltip: the next reset to expire and its date, how many more
 *  there are, then why nothing can be claimed right now. */
export function limitResetTooltip(
	entries: LimitResetEntry[],
	status: LimitResetStatus | null
): string {
	const lines: string[] = [];
	const next = nextResetToExpire(entries);
	if (next) {
		const title = resetTitle(next);
		const expiry = resetExpiry(next);
		lines.push(expiry ? m.limit_resets_next({ title, expiry }) : title);
		if (entries.length > 1) lines.push(m.limit_resets_more({ n: entries.length - 1 }));
	}
	if (status && !status.available) {
		const hint = limitResetHint(status);
		if (hint) lines.push(hint);
	}
	if (lines.length === 0) return status ? limitResetLabel(status) : '';
	return lines.join('\n');
}
