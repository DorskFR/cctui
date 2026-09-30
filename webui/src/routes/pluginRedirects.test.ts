import { describe, expect, it } from 'vitest';
import { isRedirect } from '@sveltejs/kit';
import { load as loadGithub } from './github/[...path]/+page';
import { load as loadReview } from './review/[...path]/+page';

type Load = typeof loadGithub;

function target(load: Load, path: string): { status: number; location: string } {
	try {
		// biome-ignore lint/suspicious/noExplicitAny: the load only reads `params`
		load({ params: { path } } as any);
	} catch (e) {
		if (isRedirect(e)) return { status: e.status, location: e.location };
		throw e;
	}
	throw new Error('expected a redirect');
}

describe.each([
	['github', loadGithub],
	['review', loadReview]
])('/%s redirects to the ghreview plugin', (_name, load) => {
	it('sends the bare route to the plugin root', () => {
		expect(target(load as Load, '')).toEqual({ status: 308, location: '/apps/ghreview' });
	});

	it('keeps the sub-path', () => {
		expect(target(load as Load, 'pr/octocat/hello/1')).toEqual({
			status: 308,
			location: '/apps/ghreview/pr/octocat/hello/1'
		});
	});
});
