import { redirect } from '@sveltejs/kit';
import { settingsHref } from '#lib/components/organisms/settings/settings.logic.ts';

export const prerender = false;

export const load = () => {
	redirect(308, settingsHref('accounts', 'provider-accounts'));
};
