import type { WebhookDelivery } from '@bindings/WebhookDelivery';
import type { WebhookOutcome } from '@bindings/WebhookOutcome';

export type DeliveryTone = 'ok' | 'muted' | 'danger' | 'warn';
export type DeliveryLabelKey =
	| 'webhooks_outcome_dispatched'
	| 'webhooks_outcome_duplicate'
	| 'webhooks_outcome_rejected'
	| 'webhooks_outcome_rate_limited';

export interface DeliveryRow {
	id: number;
	outcome: WebhookOutcome;
	tone: DeliveryTone;
	labelKey: DeliveryLabelKey;
	at: number;
	deliveryId: string | null;
	detail: string | null;
	sessionId: string | null;
}

const TONE: Record<WebhookOutcome, DeliveryTone> = {
	dispatched: 'ok',
	duplicate: 'muted',
	rejected: 'danger',
	rate_limited: 'warn'
};

export function deliveryRow(delivery: WebhookDelivery): DeliveryRow {
	return {
		id: delivery.id,
		outcome: delivery.outcome,
		tone: TONE[delivery.outcome] ?? 'muted',
		labelKey: `webhooks_outcome_${delivery.outcome}`,
		at: Date.parse(delivery.received_at),
		deliveryId: delivery.delivery_id,
		detail: delivery.detail?.trim() || null,
		sessionId: delivery.outcome === 'dispatched' ? delivery.session_id : null
	};
}

export function deliveryRows(deliveries: readonly WebhookDelivery[], limit = 25): DeliveryRow[] {
	return deliveries
		.map(deliveryRow)
		.sort((a, b) => b.at - a.at || b.id - a.id)
		.slice(0, limit);
}
