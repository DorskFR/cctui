<script lang="ts">
	import type { WebhookSecret } from '@bindings/WebhookSecret';
	import { Button, Callout, CopyButton, Field, Input, Modal, Text } from '@dorsk/tsumikit';
	import { apiBase } from '#lib/config.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { SIGNATURE_HEADER, hookUrl } from '#lib/webhooks/webhookForm.ts';

	let { issued, onclose }: { issued: WebhookSecret; onclose: () => void } = $props();

	const uid = $props.id();
	const url = $derived(hookUrl(issued.webhook, apiBase()));
</script>

<Modal open title={m.webhooks_secret_title({ name: issued.webhook.name })} {onclose} size="md">
	{#snippet body()}
		<div class="secret" data-testid="webhook-secret">
			<Callout tone="warn" icon="warning">{m.webhooks_secret_once()}</Callout>
			<Field label={m.webhooks_secret_label()} for="{uid}-secret">
				<div class="line">
					<Input id="{uid}-secret" mono readonly grow value={issued.secret} />
					<CopyButton text={issued.secret} label={m.webhooks_secret_copy()} />
				</div>
			</Field>
			<Field label={m.webhooks_url_label()} for="{uid}-url">
				<div class="line">
					<Input id="{uid}-url" mono readonly grow value={url} />
					<CopyButton text={url} label={m.webhooks_url_copy()} />
				</div>
			</Field>
			<Text size="sm" tone="muted">{m.webhooks_sign_hint({ header: SIGNATURE_HEADER })}</Text>
		</div>
	{/snippet}
	{#snippet footer()}
		<Button variant="primary" onclick={onclose}>{m.webhooks_secret_done()}</Button>
	{/snippet}
</Modal>

<style>
	.secret {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.line {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
