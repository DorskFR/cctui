/** Compact token count: 1234 → "1.2k", 1_200_000 → "1.2M". */
export function compact(n: number): string {
	if (n < 1000) return String(n);
	if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
	return `${(n / 1_000_000).toFixed(1)}M`;
}

/** "3m ago", "2h ago", "5d ago" from an ISO datetime (or null → ""). */
export function relativeTime(iso: string | null | undefined): string {
	if (!iso) return '';
	const then = new Date(iso).getTime();
	if (Number.isNaN(then)) return '';
	const secs = Math.max(0, Math.floor((Date.now() - then) / 1000));
	if (secs < 60) return `${secs}s ago`;
	const mins = Math.floor(secs / 60);
	if (mins < 60) return `${mins}m ago`;
	const hrs = Math.floor(mins / 60);
	if (hrs < 24) return `${hrs}h ago`;
	const days = Math.floor(hrs / 24);
	if (days < 30) return `${days}d ago`;
	return new Date(iso).toLocaleDateString();
}

/**
 * Relative time toward a FUTURE instant: "resets in 2h 14m", "resets in 45m".
 * `relativeTime` only counts backward (it clamps `now - then` at 0), so feeding
 * it a future `resets_at` always yields "0s ago" — this is its forward twin for
 * reset/expiry timestamps (CCT-324). Returns "" on null/invalid, and "now" once
 * the instant is in the past (the window has rolled over).
 */
export function relativeFuture(iso: string | null | undefined): string {
	if (!iso) return '';
	const then = new Date(iso).getTime();
	if (Number.isNaN(then)) return '';
	const secs = Math.floor((then - Date.now()) / 1000);
	if (secs <= 0) return 'now';
	const mins = Math.floor(secs / 60);
	if (mins < 1) return 'in <1m';
	if (mins < 60) return `in ${mins}m`;
	const hrs = Math.floor(mins / 60);
	if (hrs < 24) {
		const rem = mins % 60;
		return rem ? `in ${hrs}h ${rem}m` : `in ${hrs}h`;
	}
	const days = Math.floor(hrs / 24);
	const remH = hrs % 24;
	return remH ? `in ${days}d ${remH}h` : `in ${days}d`;
}

/** Full ISO-8601 of an ISO datetime (or "—" when null/invalid). */
function isoOrDash(iso: string | null | undefined): string {
	if (!iso) return '—';
	const d = new Date(iso);
	return Number.isNaN(d.getTime()) ? '—' : d.toISOString();
}

/**
 * Multi-line tooltip body for a relative timestamp (CCT-270): ISO start and
 * last-message datetimes, used as a native `title` so hover shows the richer
 * info with no layout shift. `lastActivity` is optional.
 */
export function timestampTooltip(
	startedAt: string | null | undefined,
	lastMessageAt: string | null | undefined,
	lastActivityAt?: string | null | undefined
): string {
	const lines = [`Started:      ${isoOrDash(startedAt)}`, `Last message: ${isoOrDash(lastMessageAt)}`];
	if (lastActivityAt) lines.push(`Last activity: ${isoOrDash(lastActivityAt)}`);
	return lines.join('\n');
}

/** Wall-clock HH:MM:SS from a unix-millis timestamp (matches the proto `AgentEvent.ts`). */
export function clockTime(tsMs: number): string {
	if (!Number.isFinite(tsMs)) return '';
	return new Date(tsMs).toLocaleTimeString([], {
		hour: '2-digit',
		minute: '2-digit',
		second: '2-digit'
	});
}

/** Human uptime from seconds: "2d 3h", "4h 10m", "12m". */
export function uptime(secs: number): string {
	const d = Math.floor(secs / 86400);
	const h = Math.floor((secs % 86400) / 3600);
	const m = Math.floor((secs % 3600) / 60);
	if (d > 0) return `${d}d ${h}h`;
	if (h > 0) return `${h}h ${m}m`;
	return `${m}m`;
}

/** Short calendar date from ISO. */
export function dateOnly(iso: string | null | undefined): string {
	if (!iso) return '—';
	const d = new Date(iso);
	return Number.isNaN(d.getTime()) ? '—' : d.toLocaleDateString();
}

/** Map a session status to a badge color class (active = green, etc.). */
export function statusBadgeClass(status: string): string {
	switch (status) {
		case 'active':
			return 'badge-ok';
		case 'new':
			return 'badge-info';
		case 'archived':
			return 'badge-danger';
		default:
			return ''; // inactive / dead → neutral grey
	}
}

/** Deterministic accent color for a machine label (badge tinting). */
export function hashHue(s: string): number {
	let h = 0;
	for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
	return h % 360;
}
