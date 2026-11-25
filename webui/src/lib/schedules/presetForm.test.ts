import { describe, expect, it } from 'vitest';
import type { LaunchPreset } from '@bindings/LaunchPreset';
import { withExternalSources } from '#lib/webhooks/externalSources.ts';
import {
	emptyForm,
	formFromPreset,
	formProblems,
	hasProblems,
	isSchedule,
	requestFromForm,
	requestFromPreset,
	timezoneOptions,
	withEnabled
} from './presetForm';

const template = {
	effort: 'high',
	service_tier: null,
	label_ids: ['l1'],
	context: { items: ['notes'], auto: false },
	profile_id: 'prof-1',
	auto_archive: true,
	attachment_names: ['brief.md']
};

const preset = (over: Partial<LaunchPreset> = {}): LaunchPreset => ({
	id: 'p1',
	user_id: 'u1',
	name: 'Morning report',
	prompt: 'Summarise the night',
	machine_id: 'm1',
	working_dir: '/home/alice/widgets',
	adapter_id: 'claude-code',
	model: 'opus',
	permission_mode: 'auto',
	account_id: 'a1',
	pool_id: null,
	no_account: false,
	dispatcher_id: null,
	profile: null,
	external_sources: true,
	fresh_secs: 900,
	on_stale: 'new',
	max_batch: 4,
	cron: '0 7 * * 1-5',
	timezone: 'Asia/Tokyo',
	enabled: true,
	overlap: 'skip',
	next_fire_at: '2026-11-20T22:00:00Z',
	last_fire_at: null,
	then_preset_id: 'p2',
	notify_url: null,
	env_keys: ['FEED_TOKEN'],
	template,
	created_at: '2026-11-01T00:00:00Z',
	updated_at: '2026-11-01T00:00:00Z',
	...over
});

const dispatcherPreset = () =>
	preset({
		machine_id: null,
		working_dir: null,
		adapter_id: null,
		model: null,
		permission_mode: null,
		account_id: null,
		dispatcher_id: 'd1',
		profile: 'review',
		notify_url: 'https://hooks.example/done',
		template: {}
	});

const kept = { env_keys: ['FEED_TOKEN'], env: {} };

describe('the enabled toggle', () => {
	it('re-sends every field of the preset with enabled flipped, template included', () => {
		const p = preset();
		const body = withEnabled(p, false);
		expect(body).toEqual({ ...requestFromPreset(p), enabled: false });
		expect(body).toMatchObject({ name: 'Morning report', cron: '0 7 * * 1-5', then_preset_id: 'p2', template });
		expect(body.env_keys).toBeNull();
		expect(body.env).toBeNull();
	});

	it('carries no response-only field', () => {
		const body = withEnabled(preset(), true) as Record<string, unknown>;
		for (const key of ['id', 'user_id', 'next_fire_at', 'last_fire_at', 'created_at', 'updated_at']) {
			expect(body).not.toHaveProperty(key);
		}
	});
});

describe('the external-sources round trip', () => {
	it('sends the GET object back with only external_sources flipped, template and env names kept', () => {
		const p = preset({ external_sources: false });
		const body = withExternalSources(p) as Record<string, unknown>;
		expect(body.external_sources).toBe(true);
		expect(body.template).toEqual(template);
		expect(body.env_keys).toEqual(['FEED_TOKEN']);
		expect(body).not.toHaveProperty('env');
		const { external_sources: _a, ...rest } = body;
		const { external_sources: _b, env_keys: _c, env: _d, ...fromForm } = requestFromPreset(p);
		expect(rest).toMatchObject(fromForm);
	});
});

describe('the spawn template', () => {
	it('opens a schedule as the spawn form state: where, prompt, kit, labels and context', () => {
		const form = formFromPreset(preset(), 'UTC');
		expect(form.spawn).toMatchObject({
			machine_id: 'm1',
			working_dir: '/home/alice/widgets',
			name: 'Morning report',
			prompt: 'Summarise the night',
			labels: ['l1'],
			context_items: ['notes'],
			context_auto: false,
			auto_archive: true
		});
		expect(form.kit).toEqual({
			harness: 'claude-code',
			account_id: 'a1',
			pool_id: null,
			no_account: false,
			model_alias: 'opus',
			effort: 'high',
			permission_mode: 'auto',
			service_tier: null
		});
	});

	it('round-trips an unedited preset, every stored env value kept by name', () => {
		const p = preset();
		expect(requestFromForm(formFromPreset(p, 'UTC'), { schedule: true, base: p })).toEqual({
			...requestFromPreset(p),
			...kept
		});
	});

	it('writes the edited kit, labels and context into the request', () => {
		const p = preset();
		const form = formFromPreset(p, 'UTC');
		form.kit = { ...form.kit, harness: 'codex', model_alias: 'gpt-5', effort: 'low', service_tier: 'fast', account_id: null, pool_id: 'pl1' };
		form.spawn.labels = ['l2'];
		form.spawn.context_items = [];
		const body = requestFromForm(form, { schedule: true, base: p });
		expect(body).toMatchObject({ adapter_id: 'codex', model: 'gpt-5', account_id: null, pool_id: 'pl1', no_account: false });
		expect(body.template).toEqual({
			...template,
			effort: 'low',
			service_tier: 'fast',
			label_ids: ['l2'],
			context: null
		});
	});

	it('writes the auto-archive tick into the template, both ways', () => {
		const p = preset();
		const form = formFromPreset(p, 'UTC');
		form.spawn.auto_archive = false;
		expect(requestFromForm(form, { schedule: true, base: p }).template?.auto_archive).toBe(false);
		const fresh = emptyForm({ machine_id: 'm1', timezone: 'UTC' });
		fresh.spawn.auto_archive = true;
		expect(requestFromForm(fresh, { schedule: true }).template?.auto_archive).toBe(true);
	});

	it('builds a new schedule from the form', () => {
		const form = emptyForm({ machine_id: 'm1', timezone: 'Asia/Tokyo' });
		form.spawn.name = ' Standup ';
		form.spawn.prompt = 'Prepare the standup';
		form.kit.effort = 'high';
		form.repeat = { kind: 'weekdays', time: '07:00', weekdays: [1], custom: '' };
		const body = requestFromForm(form, { schedule: true });
		expect(body).toMatchObject({
			name: 'Standup',
			machine_id: 'm1',
			working_dir: '~',
			adapter_id: 'claude-code',
			cron: '0 7 * * 1-5',
			timezone: 'Asia/Tokyo',
			env_keys: [],
			env: {}
		});
		expect(body.template).toMatchObject({ effort: 'high', label_ids: [], context: null, attachment_names: [] });
	});

	it('never invents a machine target or a template for a dispatcher preset', () => {
		const p = dispatcherPreset();
		const form = formFromPreset(p, 'UTC');
		form.spawn.machine_id = 'm9';
		form.kit.effort = 'high';
		form.enabled = false;
		expect(requestFromForm(form, { schedule: true, base: p })).toEqual({
			...requestFromPreset(p),
			enabled: false,
			...kept
		});
	});

	it('leaves the cron out of a preset edited without the schedule section', () => {
		const form = emptyForm({ machine_id: 'm1', timezone: 'Asia/Tokyo' });
		form.spawn.name = 'Hook';
		form.spawn.prompt = 'Handle it';
		expect(requestFromForm(form, { schedule: false })).toMatchObject({ cron: null, timezone: null, enabled: null, overlap: null });
	});
});

describe('the schedule env', () => {
	it('shows stored names masked and never a value', () => {
		const form = formFromPreset(preset({ env_keys: ['FEED_TOKEN', 'API_KEY'] }), 'UTC');
		expect(form.env).toEqual([
			{ key: 'FEED_TOKEN', value: '', stored: true },
			{ key: 'API_KEY', value: '', stored: true }
		]);
		expect(emptyForm({ timezone: 'UTC' }).env).toEqual([]);
	});

	it('keeps, replaces, removes and adds per name', () => {
		const p = preset({ env_keys: ['KEEP', 'SWAP', 'GONE'] });
		const form = formFromPreset(p, 'UTC');
		form.env = form.env
			.filter((r) => r.key !== 'GONE')
			.map((r) => (r.key === 'SWAP' ? { ...r, value: 'new-swap' } : r));
		form.env.push({ key: 'ADDED', value: 'new-added' });
		const body = requestFromForm(form, { schedule: true, base: p });
		expect(body.env_keys).toEqual(['KEEP', 'SWAP', 'ADDED']);
		expect(body.env).toEqual({ SWAP: 'new-swap', ADDED: 'new-added' });
	});
});

describe('formProblems', () => {
	const valid = () => {
		const form = emptyForm({ machine_id: 'm1', timezone: 'UTC' });
		form.spawn.name = 'n';
		form.spawn.prompt = 'p';
		return form;
	};

	it('passes a complete form', () => {
		expect(hasProblems(formProblems(valid(), { schedule: true }))).toBe(false);
	});

	it('names every missing field', () => {
		const form = emptyForm({ timezone: 'UTC' });
		form.spawn.working_dir = 'relative/dir';
		form.kit.harness = '';
		expect(formProblems(form, { schedule: true })).toEqual({
			name: 'required',
			prompt: 'required',
			machine: 'required',
			working_dir: 'absolute',
			adapter: 'required'
		});
	});

	it('asks nothing about the machine of a dispatcher preset', () => {
		expect(formProblems(formFromPreset(dispatcherPreset(), 'UTC'), { schedule: true, dispatcher: true })).toEqual({});
	});

	it('requires a machine target that accepts external sources to run guarded', () => {
		const form = valid();
		for (const mode of [null, 'yolo', 'whip']) {
			form.kit.permission_mode = mode;
			expect(formProblems(form, { schedule: false, external: true })).toEqual({ permission_mode: 'guarded' });
		}
		form.kit.permission_mode = 'ask';
		expect(formProblems(form, { schedule: false, external: true })).toEqual({});
	});

	it('checks a custom cron only with the schedule section, and env names always', () => {
		const form = valid();
		form.repeat = { kind: 'custom', time: '09:00', weekdays: [1], custom: '0 9 * *' };
		expect(formProblems(form, { schedule: true }).cron).toEqual({ kind: 'fields', count: 4 });
		expect(formProblems(form, { schedule: false })).toEqual({});
		form.env = [{ key: 'lower-case', value: 'x' }];
		expect(formProblems(form, { schedule: false })).toEqual({ env: 'invalid' });
	});
});

describe('lists', () => {
	it('counts a preset as a schedule when it has a cron', () => {
		expect(isSchedule(preset())).toBe(true);
		expect(isSchedule(preset({ cron: null }))).toBe(false);
	});

	it('leads the timezones with UTC and keeps an unlisted current one', () => {
		expect(timezoneOptions(['Europe/Paris', 'Asia/Tokyo'], 'Mars/Olympus')).toEqual([
			'UTC',
			'Asia/Tokyo',
			'Europe/Paris',
			'Mars/Olympus'
		]);
	});
});
