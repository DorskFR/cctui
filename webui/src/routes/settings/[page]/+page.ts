import {
	SETTINGS_PAGES,
	settingsPageFeature
} from '$lib/components/organisms/settings/settings.logic';
import { requireFeature } from '$lib/workspaceSupport';

export const entries = () => SETTINGS_PAGES.map((page) => ({ page }));

export const load = async ({ params }: { params: { page: string } }) => {
	await requireFeature(settingsPageFeature(params.page));
};
