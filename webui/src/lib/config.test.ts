import { afterEach, describe, expect, it } from 'vitest';
import { ghreviewUrl } from './config';

afterEach(() => {
	delete window.CCTUI_CONFIG;
});

describe('ghreviewUrl (connector config resolution)', () => {
	it('is null when unconfigured', () => {
		expect(ghreviewUrl()).toBeNull();
	});

	it('is null when set to an empty string', () => {
		window.CCTUI_CONFIG = { ghreviewUrl: '' };
		expect(ghreviewUrl()).toBeNull();
	});

	it('returns the origin without a trailing slash', () => {
		window.CCTUI_CONFIG = { ghreviewUrl: 'https://ghreview.example/' };
		expect(ghreviewUrl()).toBe('https://ghreview.example');
	});
});
