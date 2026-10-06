import { redirect } from '@sveltejs/kit';
import { MOVED_ROUTES } from '../movedRoutes';

export const prerender = false;

export const load = () => {
	redirect(308, MOVED_ROUTES['/access']);
};
