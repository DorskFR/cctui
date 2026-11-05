// A static index of every Settings section, so the search box finds a
// section by name or topic even when its rows are not plain setting rows
// (passkeys, provider accounts, users) or are not mounted yet.
import { m } from '$lib/paraglide/messages';
import { fuzzyMatches, SETTINGS_ANCHORS, type SettingsPage } from './settings.logic';

type Anchor = (typeof SETTINGS_ANCHORS)[SettingsPage][number];

interface SectionEntry {
	page: SettingsPage;
	anchor: Anchor;
	title: () => string;
	keywords?: () => string;
}

export interface SectionHit {
	page: SettingsPage;
	anchor: string;
	area: string;
	title: string;
}

export const AREA_LABELS: Record<SettingsPage, () => string> = {
	interface: m.set_nav_interface,
	accounts: m.set_nav_accounts,
	sessions: m.settings_nav_sessions,
	notifications: m.set_nav_notifications,
	plugins: m.settings_nav_plugins,
	instance: m.settings_nav_instance
};

export const SETTINGS_SECTIONS: readonly SectionEntry[] = [
	{ page: 'interface', anchor: 'theme', title: m.set_theme_title, keywords: m.set_kw_theme },
	{ page: 'interface', anchor: 'layout', title: m.set_layout_title, keywords: m.set_kw_layout },
	{ page: 'interface', anchor: 'monitoring', title: m.set_monitoring_title },
	{ page: 'interface', anchor: 'conversation-display', title: m.set_conversation_display_title },
	{ page: 'interface', anchor: 'guides', title: m.settings_guides_title },
	{ page: 'interface', anchor: 'storage', title: m.set_storage_title },
	{ page: 'accounts', anchor: 'profile', title: m.set_profile_title, keywords: m.set_kw_profile },
	{ page: 'accounts', anchor: 'security', title: m.set_security_title, keywords: m.set_kw_security },
	{ page: 'accounts', anchor: 'machines', title: m.set_my_machines_title, keywords: m.set_kw_machines },
	{ page: 'accounts', anchor: 'providers', title: m.set_providers_title, keywords: m.set_kw_providers },
	{ page: 'sessions', anchor: 'profiles', title: m.set_launch_title, keywords: m.set_kw_launch },
	{ page: 'sessions', anchor: 'macros', title: m.settings_nav_macros },
	{ page: 'sessions', anchor: 'conversation', title: m.settings_group_conversation },
	{ page: 'sessions', anchor: 'context', title: m.settings_nav_context, keywords: m.set_kw_context },
	{ page: 'sessions', anchor: 'privacy', title: m.settings_nav_privacy, keywords: m.set_kw_privacy },
	{ page: 'sessions', anchor: 'automation', title: m.set_automation_title, keywords: m.set_kw_automation },
	{ page: 'notifications', anchor: 'notifications', title: m.settings_notifications_title },
	{ page: 'notifications', anchor: 'voice', title: m.settings_voice_title },
	{ page: 'plugins', anchor: 'plugins', title: m.settings_nav_plugins },
	{ page: 'instance', anchor: 'general', title: m.set_general_title, keywords: m.set_kw_general },
	{ page: 'instance', anchor: 'users', title: m.settings_nav_users, keywords: m.set_kw_users },
	{ page: 'instance', anchor: 'dispatchers', title: m.settings_nav_dispatchers },
	{ page: 'instance', anchor: 'harnesses', title: m.set_harnesses_title, keywords: m.set_kw_harnesses },
	{ page: 'instance', anchor: 'limits', title: m.set_limits_title, keywords: m.set_kw_limits },
	{ page: 'instance', anchor: 'network', title: m.set_network_title, keywords: m.set_kw_network },
	{ page: 'instance', anchor: 'integrations', title: m.set_integrations_title, keywords: m.set_kw_integrations },
	{ page: 'instance', anchor: 'instance-plugins', title: m.set_instance_plugins_title, keywords: m.set_kw_instance_plugins }
];

/** Sections whose area, title or topic words match the query; Instance only for admins. */
export function searchSections(query: string, isAdmin: boolean): SectionHit[] {
	if (!query.trim()) return [];
	return SETTINGS_SECTIONS.filter((s) => isAdmin || s.page !== 'instance')
		.map((s) => ({ entry: s, area: AREA_LABELS[s.page](), title: s.title() }))
		.filter(({ entry, area, title }) =>
			fuzzyMatches(`${area} ${title} ${entry.keywords?.() ?? ''}`, query)
		)
		.map(({ entry, area, title }) => ({ page: entry.page, anchor: entry.anchor, area, title }));
}
