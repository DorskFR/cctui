import { describe, expect, it, vi } from 'vitest';
import type { LaunchPreset } from '@bindings/LaunchPreset';
import type { LaunchPresetRequest } from '@bindings/LaunchPresetRequest';
import { enableExternalSources, externalSourcesOffer, withExternalSources } from './externalSources';

const stored = {
	id: 'p1',
	user_id: 'u1',
	name: 'Nightly',
	prompt: 'triage',
	machine_id: null,
	working_dir: null,
	adapter_id: null,
	model: null,
	permission_mode: null,
	account_id: null,
	pool_id: null,
	no_account: false,
	dispatcher_id: 'd1',
	profile: 'worker',
	external_sources: false,
	fresh_secs: 600,
	on_stale: 'followup',
	max_batch: 10,
	cron: '0 3 * * *',
	timezone: 'Asia/Tokyo',
	enabled: true,
	overlap: 'skip',
	next_fire_at: '2026-11-20T03:00:00Z',
	last_fire_at: null,
	then_preset_id: null,
	notify_url: null,
	env_keys: ['API_TOKEN'],
	created_at: '2026-11-01T00:00:00Z',
	updated_at: '2026-11-02T00:00:00Z'
};

const preset = (over: Record<string, unknown> = {}): LaunchPreset =>
	({ ...stored, ...over }) as unknown as LaunchPreset;

describe('externalSourcesOffer', () => {
	it('offers nothing for a preset that already accepts external sources', () => {
		expect(externalSourcesOffer(preset({ external_sources: true }))).toEqual({ kind: 'open' });
	});

	it('offers the switch for a container preset', () => {
		expect(externalSourcesOffer(preset())).toEqual({ kind: 'enable', machine: false });
	});

	it('offers the switch with the guard note for a guarded machine preset', () => {
		for (const mode of ['ask', 'auto']) {
			const machine = preset({ machine_id: 'm1', dispatcher_id: null, permission_mode: mode });
			expect(externalSourcesOffer(machine)).toEqual({ kind: 'enable', machine: true });
		}
	});

	it('refuses the switch for an unguarded machine preset', () => {
		for (const mode of [null, 'yolo', 'plan']) {
			const machine = preset({ machine_id: 'm1', dispatcher_id: null, permission_mode: mode });
			expect(externalSourcesOffer(machine)).toEqual({ kind: 'unguarded' });
		}
	});
});

describe('enabling external sources from the webhook form', () => {
	it('sends back every stored field with only external_sources flipped', () => {
		const body = withExternalSources(preset()) as unknown as Record<string, unknown>;
		const responseOnly = ['id', 'user_id', 'next_fire_at', 'last_fire_at', 'created_at', 'updated_at'];
		const expected: Record<string, unknown> = { ...stored, external_sources: true };
		for (const key of responseOnly) delete expected[key];
		expect(body).toEqual(expected);
		for (const key of responseOnly) expect(body).not.toHaveProperty(key);
		expect(body.env_keys).toEqual(['API_TOKEN']);
	});

	it('reads the preset fresh and PUTs the round trip to it', async () => {
		const fresh = preset({ prompt: 'edited elsewhere' });
		const saved = preset({ prompt: 'edited elsewhere', external_sources: true });
		const get = vi.fn(async () => fresh);
		const put = vi.fn(async (_id: string, _body: LaunchPresetRequest) => saved);
		await expect(enableExternalSources('p1', { get, put })).resolves.toBe(saved);
		expect(get).toHaveBeenCalledWith('p1');
		expect(put).toHaveBeenCalledTimes(1);
		const [id, body] = put.mock.calls[0];
		expect(id).toBe('p1');
		expect(body).toEqual(withExternalSources(fresh));
		expect(body).toMatchObject({ prompt: 'edited elsewhere', external_sources: true, cron: '0 3 * * *' });
	});

	it('skips the PUT when the preset was opened meanwhile', async () => {
		const open = preset({ external_sources: true });
		const put = vi.fn();
		await expect(enableExternalSources('p1', { get: async () => open, put })).resolves.toBe(open);
		expect(put).not.toHaveBeenCalled();
	});
});
