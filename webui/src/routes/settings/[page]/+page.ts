import { redirect } from '@sveltejs/kit';
import {
	settingsPageFeature,
	settingsRedirect,
	settingsRouteSlugs
} from '#lib/components/organisms/settings/settings.logic.ts';
import { requireFeature } from '#lib/workspaceSupport.ts';

export const entries = () => settingsRouteSlugs().map((page) => ({ page }));

export const load = async ({ params }: { params: { page: string } }) => {
	const target = settingsRedirect(params.page);
	if (target) redirect(308, target);
	await requireFeature(settingsPageFeature(params.page));
};
