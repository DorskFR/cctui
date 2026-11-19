import { describe, expect, it } from 'vitest';
import type { LaunchPreset } from '@bindings/LaunchPreset';
import { NO_ACCOUNT, POOL_PREFIX } from '#lib/components/organisms/spawn/options.ts';
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

const preset = (over: Partial<LaunchPreset> = {}): LaunchPreset => ({
	id: 'p1',
	user_id: 'u1',
	name: 'Morning report',
	prompt: 'Summarise the night',
	machine_id: 'm1',
	working_dir: '/home/alice/widgets',
	adapter_id: 'claude',
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
		notify_url: 'https://hooks.example/done'
	});

describe('the enabled toggle', () => {
	it('re-sends every field of the preset with enabled flipped', () => {
		const p = preset();
		const body = withEnabled(p, false);
		expect(body).toEqual({ ...requestFromPreset(p), enabled: false });
		expect(body).toMatchObject({
			name: 'Morning report',
			cron: '0 7 * * 1-5',
			timezone: 'Asia/Tokyo',
			external_sources: true,
			fresh_secs: 900,
			on_stale: 'new',
			max_batch: 4,
			then_preset_id: 'p2',
			overlap: 'skip'
		});
	});

	it('carries no response-only field', () => {
		const body = withEnabled(preset(), true) as Record<string, unknown>;
		for (const key of ['id', 'user_id', 'next_fire_at', 'last_fire_at', 'created_at', 'updated_at']) {
			expect(body).not.toHaveProperty(key);
		}
	});

	it('keeps a dispatcher target', () => {
		expect(withEnabled(dispatcherPreset(), false)).toMatchObject({
			dispatcher_id: 'd1',
			profile: 'review',
			machine_id: null,
			notify_url: 'https://hooks.example/done',
			enabled: false
		});
	});
});

describe('requestFromForm', () => {
	it('builds a new weekday schedule', () => {
		const form = emptyForm({ machine_id: 'm1', adapter_id: 'claude', timezone: 'Asia/Tokyo' });
		form.name = ' Standup ';
		form.prompt = 'Prepare the standup';
		form.repeat = { kind: 'weekdays', time: '07:00', weekdays: [1], custom: '' };
		expect(requestFromForm(form, { schedule: true })).toEqual({
			name: 'Standup',
			prompt: 'Prepare the standup',
			machine_id: 'm1',
			working_dir: '~',
			adapter_id: 'claude',
			model: null,
			permission_mode: null,
			account_id: null,
			pool_id: null,
			no_account: false,
			dispatcher_id: null,
			profile: null,
			external_sources: false,
			fresh_secs: null,
			on_stale: null,
			max_batch: null,
			cron: '0 7 * * 1-5',
			timezone: 'Asia/Tokyo',
			enabled: true,
			overlap: 'skip',
			then_preset_id: null,
			notify_url: null
		});
	});

	it('leaves the cron out of a preset edited without the schedule section', () => {
		const form = emptyForm({ machine_id: 'm1', adapter_id: 'claude', timezone: 'Asia/Tokyo' });
		form.name = 'Hook';
		form.prompt = 'Handle it';
		expect(requestFromForm(form, { schedule: false })).toMatchObject({
			cron: null,
			timezone: null,
			enabled: null,
			overlap: null
		});
	});

	it('round-trips an unedited preset', () => {
		const p = preset();
		expect(requestFromForm(formFromPreset(p, 'UTC'), { schedule: true, base: p })).toEqual(requestFromPreset(p));
	});

	it('round-trips a dispatcher preset and never invents a machine target', () => {
		const p = dispatcherPreset();
		const form = formFromPreset(p, 'UTC');
		form.machine_id = 'm9';
		form.adapter_id = 'codex';
		form.enabled = false;
		expect(requestFromForm(form, { schedule: true, base: p })).toEqual({
			...requestFromPreset(p),
			enabled: false
		});
	});

	it('keeps the schedule of a preset edited without the schedule section', () => {
		const p = preset();
		const form = formFromPreset(p, 'UTC');
		form.prompt = 'Changed';
		expect(requestFromForm(form, { schedule: false, base: p })).toEqual({
			...requestFromPreset(p),
			prompt: 'Changed'
		});
	});
});

describe('the account pick', () => {
	it('round-trips an account, a pool and no account, and defaults to Auto', () => {
		const pick = (over: Partial<LaunchPreset>) => {
			const p = preset({ account_id: null, pool_id: null, no_account: false, ...over });
			const form = formFromPreset(p, 'UTC');
			return { pick: form.account, body: requestFromForm(form, { schedule: true, base: p }) };
		};
		expect(pick({ account_id: 'a1' })).toMatchObject({
			pick: 'a1',
			body: { account_id: 'a1', pool_id: null, no_account: false }
		});
		expect(pick({ pool_id: 'pl1' })).toMatchObject({
			pick: `${POOL_PREFIX}pl1`,
			body: { account_id: null, pool_id: 'pl1', no_account: false }
		});
		expect(pick({ no_account: true })).toMatchObject({
			pick: NO_ACCOUNT,
			body: { account_id: null, pool_id: null, no_account: true }
		});
		expect(pick({})).toMatchObject({ pick: '', body: { account_id: null, pool_id: null, no_account: false } });
	});

	it('replaces the stored pick with the one chosen in the form', () => {
		const p = preset({ account_id: 'a1' });
		const form = formFromPreset(p, 'UTC');
		form.account = `${POOL_PREFIX}pl1`;
		expect(requestFromForm(form, { schedule: true, base: p })).toMatchObject({
			account_id: null,
			pool_id: 'pl1',
			no_account: false
		});
	});

	it('is never sent for a dispatcher target', () => {
		const p = dispatcherPreset();
		const body = requestFromForm(formFromPreset(p, 'UTC'), { schedule: true, base: p });
		expect(body).toMatchObject({ account_id: null, pool_id: null, no_account: false });
	});
});

describe('formProblems', () => {
	const valid = () => {
		const form = emptyForm({ machine_id: 'm1', adapter_id: 'claude', timezone: 'UTC' });
		form.name = 'n';
		form.prompt = 'p';
		return form;
	};

	it('passes a complete form', () => {
		expect(hasProblems(formProblems(valid(), { schedule: true }))).toBe(false);
	});

	it('names every missing field', () => {
		const form = emptyForm({ timezone: 'UTC' });
		form.working_dir = 'relative/dir';
		expect(formProblems(form, { schedule: true })).toEqual({
			name: 'required',
			prompt: 'required',
			machine: 'required',
			working_dir: 'absolute',
			adapter: 'required'
		});
	});

	it('asks nothing about the machine of a dispatcher preset', () => {
		const form = formFromPreset(dispatcherPreset(), 'UTC');
		expect(formProblems(form, { schedule: true, dispatcher: true })).toEqual({});
	});

	it('checks a custom cron only with the schedule section', () => {
		const form = valid();
		form.repeat = { kind: 'custom', time: '09:00', weekdays: [1], custom: '0 9 * *' };
		expect(formProblems(form, { schedule: true }).cron).toEqual({ kind: 'fields', count: 4 });
		expect(formProblems(form, { schedule: false })).toEqual({});
	});

	it('wants a day for a weekly repeat and a time for a timed one', () => {
		const form = valid();
		form.repeat = { kind: 'weekly', time: '', weekdays: [], custom: '' };
		expect(formProblems(form, { schedule: true })).toEqual({ time: 'invalid', weekdays: 'required' });
	});
});

describe('a preset that accepts external sources', () => {
	const form = () => {
		const f = emptyForm({ machine_id: 'm1', adapter_id: 'claude', timezone: 'UTC' });
		f.name = 'Issue triage';
		f.prompt = 'Triage it';
		return f;
	};

	it('is sent with external_sources on and no cron', () => {
		const f = form();
		f.permission_mode = 'auto';
		expect(requestFromForm(f, { schedule: false, external: true })).toMatchObject({
			external_sources: true,
			permission_mode: 'auto',
			cron: null
		});
		expect(requestFromForm(f, { schedule: false }).external_sources).toBe(false);
	});

	it('requires a machine target to run guarded', () => {
		const f = form();
		for (const mode of [null, 'yolo', 'whip'] as const) {
			f.permission_mode = mode;
			expect(formProblems(f, { schedule: false, external: true })).toEqual({ permission_mode: 'guarded' });
		}
		for (const mode of ['ask', 'auto'] as const) {
			f.permission_mode = mode;
			expect(formProblems(f, { schedule: false, external: true })).toEqual({});
		}
		f.permission_mode = 'yolo';
		expect(formProblems(f, { schedule: false })).toEqual({});
	});

	it('asks nothing about the permission mode of a dispatcher target', () => {
		const f = formFromPreset(dispatcherPreset(), 'UTC');
		expect(formProblems(f, { schedule: false, dispatcher: true, external: true })).toEqual({});
	});
});

describe('formFromPreset', () => {
	it('recognises the cron and keeps the timezone', () => {
		expect(formFromPreset(preset(), 'Europe/Paris')).toMatchObject({
			repeat: { kind: 'weekdays', time: '07:00' },
			timezone: 'Asia/Tokyo',
			enabled: true,
			overlap: 'skip'
		});
	});

	it('offers the browser timezone to a preset that has no cron yet', () => {
		expect(formFromPreset(preset({ cron: null, timezone: 'UTC' }), 'Europe/Paris').timezone).toBe('Europe/Paris');
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
