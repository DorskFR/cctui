import { m } from '$lib/paraglide/messages';

export const ALL_SCOPES = ['read', 'dispatch', 'enroll', 'admin'] as const;
export type ScopeName = (typeof ALL_SCOPES)[number];

export interface ScopeCell {
	name: ScopeName;
	granted: boolean;
}

export function scopeCells(granted: readonly string[]): ScopeCell[] {
	const held = new Set(granted);
	return ALL_SCOPES.map((name) => ({ name, granted: held.has(name) }));
}

export interface Revocable {
	revoked_at: string | null;
}

export function splitRevoked<T extends Revocable>(
	rows: readonly T[]
): { active: T[]; revoked: T[] } {
	const active: T[] = [];
	const revoked: T[] = [];
	for (const r of rows) (r.revoked_at ? revoked : active).push(r);
	return { active, revoked };
}

export function visibleRows<T extends Revocable>(rows: readonly T[], showRevoked: boolean): T[] {
	const { active, revoked } = splitRevoked(rows);
	return showRevoked ? [...active, ...revoked] : active;
}

export function filterByName<T extends { name: string }>(rows: readonly T[], query: string): T[] {
	const q = query.trim().toLowerCase();
	return q ? rows.filter((r) => r.name.toLowerCase().includes(q)) : [...rows];
}

export const keyIcon = (kind: string): 'tv' | 'user' => (kind === 'machine' ? 'tv' : 'user');

/** A user's machine state in words: "online", "2 machines online", "offline". */
export function userStateWords(online: number, total: number): string {
	if (online <= 0) return m.set_user_state_offline();
	if (online === 1 && total === 1) return m.set_user_state_online();
	return m.set_user_state_machines_online({ count: online });
}
