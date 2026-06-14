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

/** Human uptime from seconds: "2d 3h", "4h 10m", "12m". */
export function uptime(secs: number): string {
	const d = Math.floor(secs / 86400);
	const h = Math.floor((secs % 86400) / 3600);
	const m = Math.floor((secs % 3600) / 60);
	if (d > 0) return `${d}d ${h}h`;
	if (h > 0) return `${h}h ${m}m`;
	return `${m}m`;
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

/** Short model label for the detailed footer: drop the vendor prefix
 *  ("claude-opus-4-8" → "opus-4-8", "gpt-5-codex" stays) so a long id stops
 *  shoving the provider logo out of the row. */
export function modelShort(model: string): string {
	return model.replace(/^(claude|anthropic)-/i, '');
}

/** Model FAMILY only — the one word that survives in the compact list row
 *  ("claude-opus-4-8" → "opus", "gpt-5-codex" → "gpt"). Falls back to the first
 *  segment of the (prefix-stripped) id for engines we don't enumerate. */
export function modelFamily(model: string): string {
	const m = model.toLowerCase();
	for (const fam of ['opus', 'sonnet', 'haiku', 'fable', 'gpt', 'o1', 'o3', 'o4', 'gemini', 'grok']) {
		if (m.includes(fam)) return fam;
	}
	return modelShort(model).split(/[-\s]/)[0] || model;
}

/** Deterministic accent color for a machine label (badge tinting). */
export function hashHue(s: string): number {
	let h = 0;
	for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
	return h % 360;
}
