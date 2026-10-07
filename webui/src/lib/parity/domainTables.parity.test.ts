import { describe, expect, it } from 'vitest';
import {
	END_REASONS,
	HARNESS_MODELS,
	HARNESSES,
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
	harnesses: unknown[];
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
	it('harness table', () => {
		expect(HARNESSES).toEqual(fx.harnesses);
	});
	it('an unknown harness gets only the default entry, never claude models', () => {
		const fallback = harnessModelsFallback('nonesuch');
		expect(fallback.models.map((o) => o.v)).toEqual(['']);
		expect(fallback.efforts).toEqual([]);
	});
});
