import { error } from '@sveltejs/kit';
import { ApiError } from '#lib/api.ts';
import { endpoints } from '#lib/queries/endpoints.ts';
import type { PageLoad } from './$types';

/** A deleted session keeps its URL and lands on the error page; any other
 *  failure is left to the sessions page, which can retry. */
export const load: PageLoad = async ({ params }) => {
	try {
		await endpoints.session(params.session);
	} catch (e) {
		if (e instanceof ApiError && e.status === 404) error(404, { message: 'session_missing' });
	}
};
