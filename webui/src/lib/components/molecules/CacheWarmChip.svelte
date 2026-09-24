<script lang="ts">
	import { onMount } from 'svelte';
	import { Badge, Timestamp } from '@dorsk/tsumikit';
	import type { WarmSession } from '$lib/components/organisms/conversation/keepalive';
	import { keepaliveActive, warmUntilMs } from '$lib/components/organisms/conversation/keepalive';
	import { m } from '$lib/paraglide/messages';

	let { session }: { session: WarmSession } = $props();

	let now = $state(Date.now());
	onMount(() => {
		const t = setInterval(() => (now = Date.now()), 15_000);
		return () => clearInterval(t);
	});

	const until = $derived(warmUntilMs(session, now));
	const ticking = $derived(!!session.keepalive && keepaliveActive(session.keepalive, now));
	const warm = $derived(until !== null && until > now);
	const indefinite = $derived(until === Number.POSITIVE_INFINITY);
</script>

{#if until !== null}
	<Badge
		tone={warm ? (ticking ? 'ok' : 'neutral') : 'warn'}
		title={ticking ? m.keepalive_chip_ticking_title() : m.keepalive_chip_title()}
	>
		<span aria-hidden="true">{warm ? '🔥' : '❄️'}</span>
		{#if !warm}
			{m.keepalive_chip_cold()}
		{:else if indefinite}
			{m.keepalive_chip_kept_warm()}
		{:else}
			{m.keepalive_chip_warm_until()}
			<Timestamp value={until} mode="time" tone="inherit" size="xs" />
		{/if}
	</Badge>
{/if}
