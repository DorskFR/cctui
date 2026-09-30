import { redirect } from '@sveltejs/kit';
import { SESSIONS_TILES_HREF } from './redirect';

export const prerender = false;

export const load = () => {
	redirect(307, SESSIONS_TILES_HREF);
};
