import type { EventRecord } from '@bindings/EventRecord';

export type EventFamily = 'session' | 'machine' | 'system' | 'other';
export type EventSeverity = 'info' | 'warn' | 'error';
export type ActorKind = 'user' | 'daemon' | 'reaper' | 'system' | 'agent';

export function kindFamily(kind: string): EventFamily {
	const head = kind.split('.')[0];
	return head === 'session' || head === 'machine' || head === 'system' ? head : 'other';
}

export function kindVerb(kind: string): string {
	const dot = kind.indexOf('.');
	return (dot < 0 ? kind : kind.slice(dot + 1)).replace(/_/g, ' ');
}

export function severityOf(ev: Pick<EventRecord, 'severity'>): EventSeverity {
	return ev.severity === 'warn' || ev.severity === 'error' ? ev.severity : 'info';
}

export function severityDot(severity: string): 'active' | 'stale' | 'dead' {
	switch (severity) {
		case 'error':
			return 'dead';
		case 'warn':
			return 'stale';
		default:
			return 'active';
	}
}

export function severityTone(severity: string): 'info' | 'warn' | 'danger' {
	switch (severity) {
		case 'error':
			return 'danger';
		case 'warn':
			return 'warn';
		default:
			return 'info';
	}
}

export function actorKind(actor: string): ActorKind {
	if (actor.startsWith('user:')) return 'user';
	if (actor.startsWith('agent:')) return 'agent';
	if (actor === 'daemon' || actor === 'reaper' || actor === 'system') return actor;
	return 'system';
}

function detailString(ev: EventRecord, key: string): string | null {
	const d = ev.detail;
	if (d === null || typeof d !== 'object' || Array.isArray(d)) return null;
	const v = (d as Record<string, unknown>)[key];
	return typeof v === 'string' && v.length > 0 ? v : null;
}

export function subjectOf(ev: EventRecord): { sessionName: string | null; machineLabel: string | null } {
	return {
		sessionName: detailString(ev, 'session_name'),
		machineLabel: detailString(ev, 'machine_label')
	};
}

/** The session id even after the row's subject was deleted (the FK goes
 *  null; the detail keeps the id as text). */
export function sessionIdOf(ev: EventRecord): string | null {
	return ev.session_id ?? detailString(ev, 'session_id');
}

/** Where the session link points: the live session page while the row still
 *  references one, nowhere once it has been deleted. */
export function sessionHref(ev: EventRecord): string | null {
	return ev.session_id === null ? null : `/sessions/${encodeURIComponent(ev.session_id)}`;
}

export interface EventFilters {
	family: 'all' | EventFamily;
	severity: 'all' | EventSeverity;
	machineId: string | null;
}

export const DEFAULT_FILTERS: EventFilters = { family: 'all', severity: 'all', machineId: null };

/** The `/events` query string for a filter set; `kind` is a family prefix. */
export function filterQuery(f: EventFilters): Record<string, string> {
	const q: Record<string, string> = {};
	if (f.family !== 'all' && f.family !== 'other') q.kind = `${f.family}.`;
	if (f.severity !== 'all') q.severity = f.severity;
	if (f.machineId) q.machine_id = f.machineId;
	return q;
}

/** The client-side twin of `filterQuery`, applied to live rows before they
 *  are prepended so a filtered feed never shows a row the list would not. */
export function matchesFilters(ev: EventRecord, f: EventFilters): boolean {
	if (f.family !== 'all' && kindFamily(ev.kind) !== f.family) return false;
	if (f.severity !== 'all' && severityOf(ev) !== f.severity) return false;
	if (f.machineId && ev.machine_id !== f.machineId) return false;
	return true;
}

/** Prepend a live row, newest first, without duplicating one the page
 *  already holds (the socket and a refetch can both deliver it). */
export function mergeLive(rows: EventRecord[], ev: EventRecord): EventRecord[] {
	if (rows.some((r) => r.id === ev.id)) return rows;
	const at = rows.findIndex((r) => r.id < ev.id);
	if (at < 0) return [...rows, ev];
	return [...rows.slice(0, at), ev, ...rows.slice(at)];
}

/** Append a fetched page below what is shown, dropping rows that a live
 *  prepend already delivered. */
export function appendPage(rows: EventRecord[], page: EventRecord[]): EventRecord[] {
	const seen = new Set(rows.map((r) => r.id));
	return [...rows, ...page.filter((r) => !seen.has(r.id))];
}

export function nextCursor(rows: EventRecord[]): number | null {
	return rows.length === 0 ? null : rows[rows.length - 1].id;
}

const MACHINE_HISTORY_KINDS = new Set([
	'machine.online',
	'machine.stale',
	'machine.offline',
	'machine.daemon_connected',
	'machine.daemon_disconnected',
	'machine.enrolled',
	'machine.updated',
	'machine.revoked',
	'machine.deleted'
]);

export function machineHistory(rows: EventRecord[]): EventRecord[] {
	return rows.filter((r) => MACHINE_HISTORY_KINDS.has(r.kind));
}

export interface EventRowView {
	id: number;
	occurredAt: string;
	family: EventFamily;
	verb: string;
	severity: EventSeverity;
	dot: 'active' | 'stale' | 'dead';
	summary: string;
	actor: ActorKind;
	sessionName: string | null;
	sessionHref: string | null;
	machineLabel: string | null;
	machineId: string | null;
}

export function formatRow(ev: EventRecord): EventRowView {
	const subject = subjectOf(ev);
	return {
		id: ev.id,
		occurredAt: ev.occurred_at,
		family: kindFamily(ev.kind),
		verb: kindVerb(ev.kind),
		severity: severityOf(ev),
		dot: severityDot(ev.severity),
		summary: ev.summary,
		actor: actorKind(ev.actor),
		sessionName: subject.sessionName,
		sessionHref: sessionHref(ev),
		machineLabel: subject.machineLabel,
		machineId: ev.machine_id
	};
}
