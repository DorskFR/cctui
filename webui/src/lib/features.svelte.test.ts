import { afterEach, describe, expect, it } from 'vitest';
import { hasFeature, routeAvailable, setCapabilities, setCapabilitiesForTest } from './features.svelte';

afterEach(() => setCapabilitiesForTest(null));

describe('announced features', () => {
	it('is on when the server announces it', () => {
		setCapabilitiesForTest({ features: ['voice'], switchable: ['voice', 'plugins'] });
		expect(hasFeature('voice')).toBe(true);
	});

	it('is off when switchable but not announced', () => {
		setCapabilitiesForTest({ features: ['voice'], switchable: ['voice', 'plugins'] });
		expect(hasFeature('plugins')).toBe(false);
	});

	it('is on when the server does not know the feature', () => {
		setCapabilitiesForTest({ features: [], switchable: ['plugins'] });
		expect(hasFeature('federation')).toBe(true);
	});

	it('is on for an older server without switchable', () => {
		setCapabilities({ workspace: true, distribution: 'full' });
		expect(hasFeature('plugins')).toBe(true);
		expect(hasFeature('voice')).toBe(true);
	});

	it('is on before capabilities load', () => {
		expect(hasFeature('plugins')).toBe(true);
	});

	it('is on when the probe failed', () => {
		setCapabilitiesForTest({ features: [], switchable: ['plugins'] });
		setCapabilities(null);
		expect(hasFeature('plugins')).toBe(true);
	});
});

describe('route availability', () => {
	it('keeps core routes and routes of an on feature', () => {
		setCapabilitiesForTest({ features: ['voice'], switchable: ['voice', 'plugins'] });
		expect(routeAvailable({ feature: null })).toBe(true);
		expect(routeAvailable({ feature: 'voice' })).toBe(true);
	});

	it('refuses routes of an off feature', () => {
		setCapabilitiesForTest({ features: [], switchable: ['plugins'] });
		expect(routeAvailable({ feature: 'plugins' })).toBe(false);
	});
});
