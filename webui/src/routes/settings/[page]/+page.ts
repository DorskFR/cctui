import { redirect } from '@sveltejs/kit';
import {
	legacySettingsTarget,
	SETTINGS_PAGES
} from '$lib/components/organisms/settings/settings.logic';

export const entries = () => SETTINGS_PAGES.map((page) => ({ page }));

export const load = ({ params }: { params: { page: string } }) => {
	const to = legacySettingsTarget(params.page);
	if (to) redirect(308, to);
};
