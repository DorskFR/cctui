import { m } from '#lib/paraglide/messages.js';

const TITLES: Record<string, () => string> = {
	theme: () => m.settings_sec_theme(),
	layout: () => m.settings_sec_layout(),
	'conversation-display': () => m.settings_sec_conversation_display(),
	help: () => m.settings_sec_help(),
	profile: () => m.settings_sec_profile(),
	security: () => m.settings_sec_security(),
	machines: () => m.settings_sec_machines(),
	'provider-accounts': () => m.settings_sec_provider_accounts(),
	pools: () => m.settings_sec_pools(),
	sharing: () => m.settings_sec_sharing(),
	launch: () => m.settings_sec_launch(),
	conversation: () => m.settings_group_conversation(),
	context: () => m.settings_nav_context(),
	privacy: () => m.settings_nav_privacy(),
	automation: () => m.settings_sec_automation(),
	notifications: () => m.settings_notifications_title(),
	voice: () => m.settings_voice_title(),
	plugins: () => m.settings_nav_plugins(),
	'instance-general': () => m.settings_sec_general(),
	'instance-users': () => m.settings_sec_users(),
	'instance-machines': () => m.settings_sec_instance_machines(),
	'instance-limits': () => m.settings_sec_limits(),
	'instance-gateway': () => m.settings_sec_gateway(),
	'instance-integrations': () => m.settings_sec_integrations(),
	'instance-plugins': () => m.settings_nav_plugins()
};

export function sectionTitle(id: string): string {
	return TITLES[id]?.() ?? id;
}
