import { compact as compactCount, usd } from '$lib/format';
import { m } from '$lib/paraglide/messages';
import { getLocale } from '$lib/paraglide/runtime';

export type StatKind = 'tokens' | 'tools' | 'files' | 'cost' | 'messages' | 'input' | 'output' | 'cache';

function count(value: number, compact: boolean): string {
	if (compact && value >= 1000) return compactCount(value).replace(/\.0(?=[kMB]$)/, '');
	return new Intl.NumberFormat(getLocale()).format(value);
}

/** A data value with its unit as a word: "25M tokens", "31 tools", "$2.38". */
export function formatStat(value: number, kind: StatKind, compact = true): string {
	const n = Number.isFinite(value) ? Math.max(0, value) : 0;
	if (kind === 'cost') return usd(n);
	const whole = Math.round(n);
	const v = count(whole, compact);
	switch (kind) {
		case 'tokens':
			return m.ds_stat_tokens({ n: whole, v });
		case 'tools':
			return m.ds_stat_tools({ n: whole, v });
		case 'files':
			return m.ds_stat_files({ n: whole, v });
		case 'messages':
			return m.ds_stat_messages({ n: whole, v });
		case 'input':
			return m.ds_stat_input({ v });
		case 'output':
			return m.ds_stat_output({ v });
		case 'cache':
			return m.ds_stat_cache({ v });
	}
}
