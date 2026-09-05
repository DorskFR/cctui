import type { TokenUsageWindows } from '@bindings/TokenUsageWindows';
import type { WindowTokenUsage } from '@bindings/WindowTokenUsage';

export type WindowKey = 'hour' | 'today' | 'day' | 'week' | 'month';

export interface WindowRow {
	key: WindowKey;
	input: number;
	output: number;
	cache_read: number;
	total: number;
	share: number;
}

export const WINDOW_KEYS: WindowKey[] = ['hour', 'today', 'day', 'week', 'month'];

const sum = (w: WindowTokenUsage | undefined) =>
	(w?.input ?? 0) + (w?.output ?? 0) + (w?.cache_read ?? 0);

export function buildWindowRows(windows: TokenUsageWindows | undefined): WindowRow[] {
	const max = sum(windows?.month) || 1;
	return WINDOW_KEYS.map((key) => {
		const w = windows?.[key];
		const total = sum(w);
		return {
			key,
			input: w?.input ?? 0,
			output: w?.output ?? 0,
			cache_read: w?.cache_read ?? 0,
			total,
			share: Math.min(1, total / max)
		};
	});
}
