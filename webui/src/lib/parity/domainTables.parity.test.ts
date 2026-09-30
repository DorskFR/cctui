import { describe, expect, it } from 'vitest';
import {
	END_REASONS,
	HARNESS_MODELS,
	PERMISSION_MODES,
	PROVIDERS,
	harnessModelsFallback
} from '$lib/domainTables';
import { parityFixture } from './fixtures';

type Fixture = {
	end_reasons: unknown[];
	providers: unknown[];
	harness_models: { harness: string }[];
	permission_modes: string[];
};

const fx = parityFixture<Fixture>('domainTables');

describe('domain table parity fixtures', () => {
	it('end reasons', () => {
		expect(END_REASONS).toEqual(fx.end_reasons);
	});
	it('providers', () => {
		expect(PROVIDERS).toEqual(fx.providers);
	});
	it('static harness model lists', () => {
		expect(HARNESS_MODELS).toEqual(fx.harness_models);
		for (const c of fx.harness_models)
			expect(harnessModelsFallback(c.harness), c.harness).toEqual(c);
	});
	it('permission modes', () => {
		expect(PERMISSION_MODES).toEqual(fx.permission_modes);
	});
	it('an unknown harness falls back to the claude shape', () => {
		expect(harnessModelsFallback('nonesuch').models).toEqual(HARNESS_MODELS[0].models);
	});
});
