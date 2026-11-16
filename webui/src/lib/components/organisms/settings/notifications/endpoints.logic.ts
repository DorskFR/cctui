import type { CreateNotificationEndpoint } from '@bindings/CreateNotificationEndpoint';
import type { NotificationEndpoint } from '@bindings/NotificationEndpoint';

const field = (config: unknown, key: string): string | null => {
	if (!config || typeof config !== 'object') return null;
	const v = (config as Record<string, unknown>)[key];
	return typeof v === 'string' && v ? v : null;
};

export const isServerEndpoint = (e: NotificationEndpoint): boolean =>
	field(e.config, 'source') === 'env';

export const hasToken = (e: NotificationEndpoint): boolean =>
	!!e.config &&
	typeof e.config === 'object' &&
	(e.config as Record<string, unknown>).has_token === true;

export function endpointSummary(e: NotificationEndpoint): string {
	if (e.channel === 'ntfy') {
		const url = field(e.config, 'url');
		const topic = field(e.config, 'topic');
		if (url && topic) return `${url.replace(/\/+$/, '')}/${topic}`;
	}
	if (e.channel === 'email') return field(e.config, 'address') ?? '';
	if (e.channel === 'web_push') {
		const endpoint = field(e.config, 'endpoint');
		if (endpoint) {
			try {
				return new URL(endpoint).host;
			} catch {
				return '';
			}
		}
	}
	return '';
}

export type NtfyDraft = { url: string; topic: string; token: string };

export function ntfyBody(d: NtfyDraft): CreateNotificationEndpoint | null {
	const url = d.url.trim();
	const topic = d.topic.trim();
	if (!url || !topic) return null;
	const token = d.token.trim();
	return { channel: 'ntfy', label: null, config: token ? { url, topic, token } : { url, topic } };
}

export function emailBody(address: string): CreateNotificationEndpoint | null {
	const a = address.trim();
	if (!/^[^\s@]+@[^\s@]+$/.test(a)) return null;
	return { channel: 'email', label: null, config: { address: a } };
}

export const listedEndpoints = (list: NotificationEndpoint[] | undefined): NotificationEndpoint[] =>
	[...(Array.isArray(list) ? list : [])].sort(
		(a, b) =>
			Number(isServerEndpoint(b)) - Number(isServerEndpoint(a)) ||
			a.channel.localeCompare(b.channel) ||
			a.created_at.localeCompare(b.created_at)
	);
