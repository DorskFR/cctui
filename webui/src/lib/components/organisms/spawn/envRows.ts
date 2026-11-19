import type { EnvRow } from './types';

export const ENV_KEY_RE = /^[A-Z_][A-Z0-9_]*$/;

export const storedEnvRows = (keys: readonly string[]): EnvRow[] =>
	keys.map((key) => ({ key, value: '', stored: true }));

/** Complete rows only (both key and value set). */
export function envMap(rows: EnvRow[]): Record<string, string> {
	const out: Record<string, string> = {};
	for (const r of rows) {
		const k = r.key.trim();
		if (k && r.value) out[k] = r.value;
	}
	return out;
}

export function envNames(rows: EnvRow[]): string[] {
	return [...new Set(rows.map((r) => r.key.trim()).filter(Boolean))];
}

/** The write-only edit the server applies: listed names keep their stored
 * value unless `env` replaces it; stored names left out are deleted. */
export function envEdit(rows: EnvRow[]): { env_keys: string[]; env: Record<string, string> } {
	return { env_keys: envNames(rows), env: envMap(rows) };
}

export const badEnvKeys = (rows: EnvRow[]): EnvRow[] =>
	rows.filter((r) => r.key.trim() && !ENV_KEY_RE.test(r.key.trim()));

/** A stored value the request would rely on without re-sending it. */
export const keepsStoredValue = (rows: EnvRow[]): boolean =>
	rows.some((r) => r.stored && r.key.trim() && !r.value);
