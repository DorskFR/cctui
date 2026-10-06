import { describe, expect, it } from 'vitest';
import config from '../journey.config.ts';

describe('journey book captures', () => {
	it('burn the step spotlight into each capture', () => {
		expect(config.presenter).toBe('spot');
	});
});
