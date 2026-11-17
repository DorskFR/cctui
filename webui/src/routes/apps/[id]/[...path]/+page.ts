import { requireFeature } from '#lib/workspaceSupport.ts';

export const prerender = false;

export const load = () => requireFeature('plugins');
