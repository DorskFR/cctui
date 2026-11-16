<script lang="ts">
	import { onMount } from 'svelte';
	import { Stack, Switch, Text } from '@dorsk/tsumikit';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { errMessage } from '$lib/api';
	import { qk } from '$lib/queries/keys';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';
	import {
		PushPermissionError,
		pushState,
		subscribe,
		unsubscribe,
		type PushState
	} from '$lib/push';

	const qc = useQueryClient();
	let status = $state<PushState | 'loading'>('loading');
	let busy = $state(false);
	const blocked = $derived(status === 'unsupported' || status === 'denied');

	async function refresh() {
		status = await pushState().catch((): PushState => 'off');
	}

	onMount(() => void refresh());

	async function toggle(on: boolean) {
		busy = true;
		try {
			if (on) await subscribe();
			else await unsubscribe();
			toasts.ok(on ? m.push_enabled() : m.push_disabled());
		} catch (e) {
			toasts.error(e instanceof PushPermissionError ? m.push_denied() : errMessage(e));
		} finally {
			await refresh();
			await qc.invalidateQueries({ queryKey: qk.notificationEndpoints });
			busy = false;
		}
	}
</script>

<Stack gap="sm">
	<Switch
		bind:checked={() => status === 'on', (v) => void toggle(v)}
		label={m.push_toggle_label()}
		labelVisible
		disabled={busy || blocked || status === 'loading'}
		data-push-toggle
	/>
	{#if status === 'unsupported'}
		<Text size="sm" tone="muted">{m.push_unsupported()}</Text>
	{:else if status === 'denied'}
		<Text size="sm" tone="muted">{m.push_denied()}</Text>
	{/if}
</Stack>
