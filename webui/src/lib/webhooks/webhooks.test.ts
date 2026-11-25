import { describe, expect, it } from 'vitest';
import type { Webhook } from '@bindings/Webhook';
import type { WebhookDelivery } from '@bindings/WebhookDelivery';
import { deliveryRow, deliveryRows } from './deliveries';
import { DEFAULT_TEMPLATE, templateExpressions } from './template';
import {
	deliveryUrl,
	emptyForm,
	formFromWebhook,
	formProblems,
	hasProblems,
	hookUrl,
	parseRate,
	parseUsd,
	parseWindow,
	presetChoices,
	requestFromForm,
	requestFromWebhook,
	withEnabled
} from './webhookForm';

const hook = (over: Partial<Webhook> = {}): Webhook => ({
	id: 'h1',
	user_id: 'u1',
	preset_id: 'p1',
	name: 'Issues',
	prompt_template: '{{event}}: {{payload.issue.title}}',
	rate_per_minute: 45,
	max_usd: null,
	budget_window_hours: 24,
	enabled: true,
	created_at: '2026-11-19T00:00:00Z',
	url: 'https://hooks.example/hooks/h1',
	...over
});

const delivery = (over: Partial<WebhookDelivery> = {}): WebhookDelivery => ({
	id: 1,
	delivery_id: 'd-1',
	outcome: 'dispatched',
	detail: null,
	event_id: 'e1',
	session_id: 'abcdef0123456789',
	received_at: '2026-11-19T09:00:00Z',
	...over
});

describe('webhook requests', () => {
	it('re-sends every field with enabled flipped', () => {
		expect(withEnabled(hook(), false)).toEqual({
			preset_id: 'p1',
			name: 'Issues',
			prompt_template: '{{event}}: {{payload.issue.title}}',
			rate_per_minute: 45,
			max_usd: null,
			budget_window_hours: 24,
			enabled: false
		});
	});

	it('carries no response-only field', () => {
		const body = requestFromWebhook(hook()) as Record<string, unknown>;
		for (const key of ['id', 'user_id', 'created_at', 'url']) expect(body).not.toHaveProperty(key);
	});

	it('round-trips an unedited hook through the form', () => {
		expect(requestFromForm(formFromWebhook(hook()))).toEqual(requestFromWebhook(hook()));
		const capped = hook({ max_usd: 12.5, budget_window_hours: 6 });
		expect(requestFromForm(formFromWebhook(capped))).toEqual(requestFromWebhook(capped));
	});

	it('builds a new hook from the form', () => {
		const form = emptyForm('p1');
		form.name = '  Issues ';
		expect(requestFromForm(form)).toEqual({
			preset_id: 'p1',
			name: 'Issues',
			prompt_template: DEFAULT_TEMPLATE,
			rate_per_minute: 30,
			max_usd: null,
			budget_window_hours: 24,
			enabled: true
		});
	});
});

describe('webhook form validation', () => {
	it('passes a complete form', () => {
		const form = emptyForm('p1');
		form.name = 'Issues';
		expect(hasProblems(formProblems(form))).toBe(false);
	});

	it('names every problem', () => {
		const form = emptyForm();
		form.prompt_template = '  ';
		form.rate_per_minute = '0';
		form.max_usd = '-2';
		form.budget_window_hours = '721';
		expect(formProblems(form)).toEqual({
			name: 'required',
			preset: 'required',
			template: 'required',
			rate: 'range',
			budget: 'range',
			window: 'range'
		});
	});

	it('holds the rate between 1 and 600 whole requests', () => {
		expect(parseRate('1')).toBe(1);
		expect(parseRate(600)).toBe(600);
		for (const bad of ['0', '601', '', '1.5', '-3', 'ten']) expect(parseRate(bad)).toBeNull();
	});
});

describe('the dollar cap', () => {
	it('reads an empty cap as none and holds amounts and windows in range', () => {
		expect(parseUsd('')).toBeNull();
		expect(parseUsd(' 5 ')).toBe(5);
		expect(parseUsd('0.25')).toBe(0.25);
		for (const bad of ['0', '-1', '1.234', 'ten', '100001']) expect(parseUsd(bad)).toBeUndefined();
		expect(parseWindow('1')).toBe(1);
		expect(parseWindow(720)).toBe(720);
		for (const bad of ['0', '721', '1.5', '']) expect(parseWindow(bad)).toBeNull();
	});

	it('sends the cap with the hook', () => {
		const form = emptyForm('p1');
		form.name = 'Issues';
		form.max_usd = '3.50';
		form.budget_window_hours = '12';
		expect(requestFromForm(form)).toMatchObject({ max_usd: 3.5, budget_window_hours: 12 });
	});
});

describe('hookUrl', () => {
	it('shows the URL the server built', () => {
		expect(hookUrl(hook(), 'http://localhost:5173/api')).toBe('https://hooks.example/hooks/h1');
	});

	it('falls back to the API base when the server sent a bare path', () => {
		expect(hookUrl(hook({ url: '/hooks/h1' }), 'https://cctui.example/api/v1')).toBe(
			'https://cctui.example/api/v1/hooks/h1'
		);
	});
});

describe('deliveryUrl', () => {
	it('puts the hook under the API base', () => {
		expect(deliveryUrl('https://cctui.example/api/v1', 'h1')).toBe('https://cctui.example/api/v1/hooks/h1');
		expect(deliveryUrl('https://cctui.example/api/v1/', 'h 1')).toBe('https://cctui.example/api/v1/hooks/h%201');
	});
});

describe('presetChoices', () => {
	it('lists every preset, the ones accepting external sources first, by name', () => {
		const presets = [
			{ id: 'a', name: 'Zeta', external_sources: true },
			{ id: 'b', name: 'Nightly', external_sources: false },
			{ id: 'c', name: 'Alpha', external_sources: true }
		];
		expect(presetChoices(presets).map((p) => p.id)).toEqual(['c', 'a', 'b']);
	});
});

describe('templateExpressions', () => {
	it('splits the expressions the server renders from the ones it leaves', () => {
		expect(
			templateExpressions('{{event}} {{ payload.issue.labels.0.name }} {{delivery}} {{payload}} {{user}} {{ env.HOME }}')
		).toEqual({
			recognised: ['event', 'payload.issue.labels.0.name', 'delivery', 'payload'],
			unrecognised: ['user', 'env.HOME']
		});
	});

	it('lists each expression once and ignores an unclosed one', () => {
		expect(templateExpressions('{{event}} {{event}} {{payload.a')).toEqual({
			recognised: ['event'],
			unrecognised: []
		});
	});

	it('reads the default template as fully recognised', () => {
		expect(templateExpressions(DEFAULT_TEMPLATE).unrecognised).toEqual([]);
	});
});

describe('delivery rows', () => {
	it('gives every outcome a tone and a label', () => {
		expect(deliveryRow(delivery())).toMatchObject({ tone: 'ok', labelKey: 'webhooks_outcome_dispatched' });
		expect(deliveryRow(delivery({ outcome: 'duplicate' }))).toMatchObject({
			tone: 'muted',
			labelKey: 'webhooks_outcome_duplicate'
		});
		expect(deliveryRow(delivery({ outcome: 'rejected' }))).toMatchObject({
			tone: 'danger',
			labelKey: 'webhooks_outcome_rejected'
		});
		expect(deliveryRow(delivery({ outcome: 'rate_limited' }))).toMatchObject({
			tone: 'warn',
			labelKey: 'webhooks_outcome_rate_limited'
		});
		expect(deliveryRow(delivery({ outcome: 'over_budget', detail: 'over budget' }))).toMatchObject({
			tone: 'warn',
			labelKey: 'webhooks_outcome_over_budget',
			detail: 'over budget',
			sessionId: null
		});
	});

	it('links a session only from a dispatched delivery', () => {
		expect(deliveryRow(delivery()).sessionId).toBe('abcdef0123456789');
		expect(deliveryRow(delivery({ session_id: null })).sessionId).toBeNull();
		expect(deliveryRow(delivery({ outcome: 'rejected', detail: ' bad signature ' }))).toMatchObject({
			sessionId: null,
			detail: 'bad signature'
		});
	});

	it('lists the newest first and caps the list', () => {
		const rows = deliveryRows(
			Array.from({ length: 30 }, (_, i) =>
				delivery({ id: i, received_at: `2026-11-${String(i + 1).padStart(2, '0')}T09:00:00Z` })
			)
		);
		expect(rows).toHaveLength(25);
		expect(rows[0].id).toBe(29);
	});
});
