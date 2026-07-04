// Drift guard for the hand-transcribed settings-catalog mirror (CCT-570).
//
// The webui carries a names-only copy of the server catalog's SAFE/CARE keys
// (settingsCatalog.ts) because there is no HTTP catalog endpoint. This test
// parses the server-side source of truth (catalog.toml) and fails when the
// mirror drifts — a missing name here means the raw-JSON box rejects a paste
// the server would accept.
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { CATALOG_KEYS, RAW_ONLY_KEYS, isKnownSettingsKey } from './settingsCatalog';

const CATALOG_TOML = join(
	dirname(fileURLToPath(import.meta.url)),
	'../../../crates/cctui-server/src/settings_catalog/catalog.toml'
);

/** Names of catalog.toml `[[keys]]` entries tagged safe/care (the exposable set). */
function serverExposableKeys(): Set<string> {
	const toml = readFileSync(CATALOG_TOML, 'utf8');
	const names = new Set<string>();
	// Each [[keys]] block is a flat run of `field = "value"` lines; blocks are
	// delimited by the next `[[` header. Good enough for this fixed format.
	for (const block of toml.split('[[keys]]').slice(1)) {
		const body = block.split('[[')[0];
		const name = body.match(/^name = "([^"]+)"/m)?.[1];
		const tag = body.match(/^tag = "([^"]+)"/m)?.[1];
		if (name && (tag === 'safe' || tag === 'care')) names.add(name);
	}
	return names;
}

describe('settingsCatalog mirror vs catalog.toml', () => {
	const server = serverExposableKeys();

	it('parses a plausible server catalog', () => {
		expect(server.size).toBeGreaterThan(50);
	});

	it('every server SAFE/CARE key is in the client allowlist', () => {
		const missing = [...server].filter((name) => !isKnownSettingsKey(name));
		expect(missing).toEqual([]);
	});

	it('the client allowlist has no keys the server would reject', () => {
		const client = [...CATALOG_KEYS.map((k) => k.name), ...RAW_ONLY_KEYS];
		const extra = client.filter((name) => !server.has(name));
		expect(extra).toEqual([]);
	});

	it('no duplicates between the toggle list and the raw-only list', () => {
		const bools = new Set(CATALOG_KEYS.map((k) => k.name));
		expect(RAW_ONLY_KEYS.filter((name) => bools.has(name))).toEqual([]);
	});
});
