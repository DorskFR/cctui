import { describe, expect, it } from 'vitest';
import { livenessClass } from './liveness';

describe('livenessClass', () => {
	it('lets hibernation win over liveness', () => {
		expect(livenessClass({ hibernated: true, liveness: 'active' })).toBe('dot-hibernated');
	});

	it('maps each liveness to its dot', () => {
		expect(livenessClass({ hibernated: false, liveness: 'active' })).toBe('dot-active');
		expect(livenessClass({ hibernated: false, liveness: 'stale' })).toBe('dot-stale');
		expect(livenessClass({ hibernated: false, liveness: 'dead' })).toBe('dot-dead');
	});
});
