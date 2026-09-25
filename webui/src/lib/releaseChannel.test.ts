import { describe, expect, it } from 'vitest';
import { releaseChannel } from './releaseChannel';

describe('releaseChannel', () => {
	it('treats plain versions as stable', () => {
		expect(releaseChannel('0.20.0')).toBe('stable');
		expect(releaseChannel('0.20.0+abc')).toBe('stable');
	});

	it('treats pre-releases and unparseable versions as beta', () => {
		expect(releaseChannel('0.21.0-beta.1')).toBe('beta');
		expect(releaseChannel('0.21.0-rc.1')).toBe('beta');
		expect(releaseChannel('dev')).toBe('beta');
	});
});
