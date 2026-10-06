import { describe, expect, it } from 'vitest';
import { releaseChannel, versionLine } from './releaseChannel';

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

describe('versionLine', () => {
	it('names one version when the UI and the server agree', () => {
		expect(versionLine('0.24.0-beta.6', '0.24.0-beta.6')).toBe('v0.24.0-beta.6');
	});
	it('names both when they differ', () => {
		expect(versionLine('0.24.0-beta.6', '0.24.0-beta.5')).toBe('ui v0.24.0-beta.6 · srv v0.24.0-beta.5');
	});
	it('falls back to the UI version before /version answers', () => {
		expect(versionLine('0.24.0', undefined)).toBe('v0.24.0');
	});
});
