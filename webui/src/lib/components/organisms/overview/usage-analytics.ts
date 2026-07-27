// Pure, reactive-free helpers for the Overview usage-analytics charts.
// Kept outside the components so the zero-fill / grid math is
// unit-testable on its own. The server buckets/extracts in the caller's
// reporting timezone (buckets arrive as UTC instants of local bucket starts),
// so all local-time derivation here mirrors that: we truncate in local time.
import type { UsageBucket } from '@bindings/UsageBucket';
import type { ModelUsage } from '@bindings/ModelUsage';
import type { HeatmapCell } from '@bindings/HeatmapCell';

export type Granularity = 'hour' | 'day';

/** A time bucket with all metrics present (missing server buckets → zeros). */
export interface FilledBucket {
	/** UTC instant (ms) of the bucket start — used as a stable key + tooltip. */
	ms: number;
	input: number;
	output: number;
	cache_read: number;
	cache_creation: number;
}

const HOUR_MS = 3_600_000;
const DAY_MS = 86_400_000;

/** Local-time truncation to the granularity floor, as epoch ms. */
function truncateLocal(d: Date, granularity: Granularity): number {
	const c = new Date(d.getTime());
	c.setMinutes(0, 0, 0);
	if (granularity === 'day') c.setHours(0);
	return c.getTime();
}

/**
 * Zero-fill the server buckets into a dense, oldest→newest series covering the
 * whole range, so the chart has no gaps. `days` + `granularity` fix the bucket
 * count (hourly: days×24, daily: days). Server rows are matched by their
 * local-truncated instant; unmatched slots are zeros.
 */
export function fillBuckets(
	rows: readonly UsageBucket[],
	days: number,
	granularity: Granularity,
	now: Date = new Date(),
): FilledBucket[] {
	const step = granularity === 'hour' ? HOUR_MS : DAY_MS;
	const count = granularity === 'hour' ? days * 24 : days;
	const byMs = new Map<number, UsageBucket>();
	for (const r of rows) {
		const ms = truncateLocal(new Date(r.bucket), granularity);
		byMs.set(ms, r);
	}
	const end = truncateLocal(now, granularity);
	const out: FilledBucket[] = [];
	for (let i = count - 1; i >= 0; i--) {
		const ms = end - i * step;
		const r = byMs.get(ms);
		out.push({
			ms,
			input: r?.input ?? 0,
			output: r?.output ?? 0,
			cache_read: r?.cache_read ?? 0,
			cache_creation: r?.cache_creation ?? 0,
		});
	}
	return out;
}

/** Peak stacked height across filled buckets (input+output+cache_read), for
 *  scaling bar heights. Never zero, so a division is always safe. */
export function peakBucketTotal(buckets: readonly FilledBucket[]): number {
	let peak = 0;
	for (const b of buckets) peak = Math.max(peak, b.input + b.output + b.cache_read);
	return peak || 1;
}

export interface ModelRow extends ModelUsage {
	/** Share of the range's total output tokens, 0–1 (for the bar width). */
	share: number;
	/** input+output+cache_read for this model. */
	total: number;
}

/**
 * Sort models by output volume (desc) and annotate each with its output share
 * for the breakdown bars. Server already orders by output desc; this stays
 * order-independent and also computes shares/totals.
 */
export function rankModels(models: readonly ModelUsage[]): ModelRow[] {
	const maxOut = models.reduce((m, r) => Math.max(m, r.output), 0) || 1;
	return [...models]
		.map((r) => ({
			...r,
			total: r.input + r.output + r.cache_read,
			share: r.output / maxOut,
		}))
		.sort((a, b) => b.output - a.output);
}

/** Fixed 7×24 grid built from the sparse heatmap cells. `[dow][hour]`, dow
 *  0=Sunday. Missing cells are zeros. `max` scales cell intensity. */
export interface HeatGrid {
	grid: { messages: number; output: number }[][];
	maxMessages: number;
}

export function buildHeatGrid(cells: readonly HeatmapCell[]): HeatGrid {
	const grid = Array.from({ length: 7 }, () =>
		Array.from({ length: 24 }, () => ({ messages: 0, output: 0 })),
	);
	let maxMessages = 0;
	for (const c of cells) {
		if (c.dow < 0 || c.dow > 6 || c.hour < 0 || c.hour > 23) continue;
		grid[c.dow][c.hour] = { messages: c.messages, output: c.output };
		maxMessages = Math.max(maxMessages, c.messages);
	}
	return { grid, maxMessages };
}

/** True when there is any usage to chart — gates the whole section. */
export function hasUsage(data: {
	buckets: readonly UsageBucket[];
	models: readonly ModelUsage[];
	heatmap: readonly HeatmapCell[];
} | undefined): boolean {
	if (!data) return false;
	return data.buckets.length > 0 || data.models.length > 0 || data.heatmap.length > 0;
}

/** Range presets → (days, granularity). Drives the selector + query. */
export const RANGES: { key: string; days: number; granularity: Granularity }[] = [
	{ key: '24h', days: 1, granularity: 'hour' },
	{ key: '7d', days: 7, granularity: 'day' },
	{ key: '30d', days: 30, granularity: 'day' },
];
