import { describe, expect, it } from 'vitest';
import { buildSpawnBody } from '$lib/components/organisms/spawn/spawnBody';
import type { Form } from '$lib/components/organisms/spawn/types';
import { parityFixture } from './fixtures';

type Case = {
	name: string;
	fields: Partial<Record<keyof Form, unknown>>;
	provider: string | null;
	expect: Record<string, unknown>;
};

const fx = parityFixture<{ cases: Case[] }>('spawnBody');

/** The fixture names only the fields a case cares about; everything else is the
 *  blank the picker opens with, which is what the Rust replay does too. */
function form(fields: Case['fields']): Form {
	const text = (key: keyof Form) => (fields[key] as string | undefined) ?? '';
	const list = (key: keyof Form) => (fields[key] as string[] | undefined) ?? [];
	return {
		machine_id: text('machine_id'),
		adapter_id: text('adapter_id'),
		working_dir: text('working_dir'),
		name: text('name'),
		prompt: text('prompt'),
		permission_mode: text('permission_mode') as Form['permission_mode'],
		dispatcher: text('dispatcher'),
		dispatch_adapter: text('dispatch_adapter'),
		identity: text('identity'),
		repo: text('repo'),
		ticket: text('ticket'),
		prompt_file: text('prompt_file'),
		model_claude: text('model_claude'),
		model_codex: text('model_codex'),
		account: text('account'),
		account_provider: text('account_provider'),
		model_account: text('model_account'),
		effort_claude: text('effort_claude'),
		effort_codex: text('effort_codex'),
		service_tier: text('service_tier'),
		timeout: text('timeout'),
		context_pack_url: text('context_pack_url'),
		context_pack_ref: text('context_pack_ref'),
		context_pack_subdir: text('context_pack_subdir'),
		context_pack_token: text('context_pack_token'),
		labels: list('labels'),
		context_items: list('context_items'),
		context_auto: (fields.context_auto as boolean | undefined) ?? false
	};
}

/** The wire omits a `false` flag and an empty list where this object spells both
 *  out, so an absent key reads as whatever empty the expectation is shaped like
 *  — the same rule `spawn_body_parity` applies on the Rust side. */
function actual(body: Record<string, unknown>, key: string, want: unknown): unknown {
	if (key in body) return body[key];
	if (typeof want === 'boolean') return false;
	if (Array.isArray(want)) return [];
	return null;
}

describe('spawnBody parity', () => {
	it('has cases', () => {
		expect(fx.cases.length).toBeGreaterThan(0);
	});

	for (const c of fx.cases) {
		it(c.name, () => {
			const body = buildSpawnBody(form(c.fields), c.provider ?? undefined, {}, null) as unknown as Record<
				string,
				unknown
			>;
			for (const [key, want] of Object.entries(c.expect)) {
				expect(actual(body, key, want), key).toEqual(want);
			}
		});
	}
});
