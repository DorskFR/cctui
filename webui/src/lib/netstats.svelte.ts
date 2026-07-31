const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const OPAQUE_RE = /^(\d+|[0-9a-f]{16,})$/i;

export function normalizeRoute(path: string): string {
	return path
		.split('/')
		.map((seg) => (UUID_RE.test(seg) || OPAQUE_RE.test(seg) ? ':id' : seg))
		.join('/');
}

export function formatBytes(n: number): string {
	if (n < 1024) return `${n} B`;
	if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
	return `${(n / (1024 * 1024)).toFixed(2)} MB`;
}

export interface RouteStat {
	route: string;
	bytes: number;
	count: number;
}

export interface WireTotals {
	/** Includes headers; 0 for cache hits. */
	transfer: number;
	encoded: number;
	decoded: number;
	count: number;
}

/** Cross-origin resource entries report 0s without
 *  `Timing-Allow-Origin`; same-origin is exact. */
export function wireTotals(): WireTotals {
	const out: WireTotals = { transfer: 0, encoded: 0, decoded: 0, count: 0 };
	if (typeof performance === 'undefined') return out;
	for (const e of performance.getEntriesByType('resource') as PerformanceResourceTiming[]) {
		if (!e.name.includes('/api/')) continue;
		out.transfer += e.transferSize;
		out.encoded += e.encodedBodySize;
		out.decoded += e.decodedBodySize;
		out.count += 1;
	}
	return out;
}

class NetStats {
	apiBytes = $state(0);
	apiCount = $state(0);
	wsBytes = $state(0);
	wsCount = $state(0);
	private byRoute = new Map<string, { bytes: number; count: number }>();
	private tick = $state(0);

	recordApi(url: string, bytes: number) {
		let path = url;
		try {
			path = new URL(url, 'http://x').pathname;
		} catch {
			path = url;
		}
		const key = normalizeRoute(path.replace(/^\/api\/v\d+/, ''));
		this.apiBytes += bytes;
		this.apiCount += 1;
		const e = this.byRoute.get(key) ?? { bytes: 0, count: 0 };
		e.bytes += bytes;
		e.count += 1;
		this.byRoute.set(key, e);
		this.tick += 1;
	}

	recordWs(bytes: number) {
		this.wsBytes += bytes;
		this.wsCount += 1;
	}

	get total(): number {
		return this.apiBytes + this.wsBytes;
	}

	routes(): RouteStat[] {
		void this.tick;
		return [...this.byRoute.entries()]
			.map(([route, s]) => ({ route, ...s }))
			.sort((a, b) => b.bytes - a.bytes);
	}
}

export const net = new NetStats();
