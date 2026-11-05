import { api } from '$lib/api';
import type { EventRetentionInfo } from '@bindings/EventRetentionInfo';
import type { EventRetentionRequest } from '@bindings/EventRetentionRequest';

export const settingsApi = {
	eventRetention: () => api.get<EventRetentionInfo>('/admin/instance/event-retention'),
	setEventRetention: (days: number | null) =>
		api.put<EventRetentionInfo>('/admin/instance/event-retention', {
			days
		} satisfies EventRetentionRequest),
	revokeMyKey: () => api.del<void>('/me/key')
};
