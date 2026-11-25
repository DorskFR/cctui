import { getLocale } from '#lib/paraglide/runtime.js';

export function fullTime(value: string | null | undefined): string | undefined {
	if (!value) return undefined;
	const d = new Date(value);
	return Number.isNaN(d.getTime()) ? undefined : d.toLocaleString(getLocale());
}
