import { redirect } from '@sveltejs/kit';
import type { PageLoad } from './$types';

export const prerender = false;

export const load: PageLoad = ({ params }) => {
	const rest = params.path ? `/${params.path}` : '';
	redirect(308, `/github${rest}`);
};
