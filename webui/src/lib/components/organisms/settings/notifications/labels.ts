import { m } from '$lib/paraglide/messages';

const CHANNELS: Record<string, () => string> = {
	in_app: m.notif_channel_in_app,
	web_push: m.notif_channel_web_push,
	ntfy: m.notif_channel_ntfy,
	email: m.notif_channel_email
};

const KINDS: Record<string, () => string> = {
	approval: m.notif_kind_approval,
	user_action: m.notif_kind_user_action,
	user_action_blocking: m.notif_kind_user_action_blocking,
	needs_input: m.notif_kind_needs_input,
	session_stuck: m.notif_kind_session_stuck,
	system_alert: m.notif_kind_system_alert,
	inbox_stuck: m.notif_kind_inbox_stuck
};

export const channelLabel = (channel: string): string => CHANNELS[channel]?.() ?? channel;
export const kindLabel = (kind: string): string => KINDS[kind]?.() ?? kind;
