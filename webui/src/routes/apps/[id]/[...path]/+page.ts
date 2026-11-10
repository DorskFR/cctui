import { requireFeature } from '$lib/workspaceSupport';

export const prerender = false;

export const load = () => requireFeature('plugins');
