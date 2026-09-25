<script lang="ts">
	import { Button, Input, Modal, Text } from '@dorsk/tsumikit';
	import { customBounds } from './scheduleTimes';
	import { m } from '$lib/paraglide/messages';

	let {
		now,
		value = $bindable(),
		onconfirm,
		onclose
	}: {
		now: number;
		value: string;
		onconfirm: () => void;
		onclose: () => void;
	} = $props();

	const bounds = $derived(customBounds(new Date(now)));
</script>

<Modal title={m.composer_schedule_custom_title()} {onclose} size="sm">
	{#snippet body()}
		<label class="custom-at">
			<Text size="sm">{m.composer_schedule_custom_label()}</Text>
			<Input type="datetime-local" min={bounds.min} max={bounds.max} bind:value onenter={onconfirm} />
		</label>
	{/snippet}
	{#snippet footer()}
		<Button onclick={onclose}>{m.common_cancel()}</Button>
		<Button variant="primary" onclick={onconfirm}>{m.composer_schedule_confirm()}</Button>
	{/snippet}
</Modal>

<style>
	.custom-at {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
</style>
