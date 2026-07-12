<script lang="ts">
	import { useCapabilities, useSessionLangfuse } from '$lib/queries';
	import { compact } from '$lib/format';
	import { Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	// Per-session Langfuse cost/usage chip (CCT-564). Shows `$cost · N calls`
	// with a deep link into Langfuse, gated on the server capability so it stays
	// hidden unless the sink is configured. Keys live only on the server: the
	// figures are proxied through `/sessions/{id}/langfuse`, the browser never
	// talks to Langfuse. Fail-open — hidden while empty or on error.
	let {
		id,
		enabled = true
	}: {
		id: string;
		/** Gate the fetch (e.g. only while the drawer is open). */
		enabled?: boolean;
	} = $props();

	const caps = useCapabilities();
	const lf = $derived($caps.data?.langfuse);
	const available = $derived(!!lf?.available);

	const q = useSessionLangfuse(
		() => id,
		() => enabled && available
	);

	const usage = $derived($q.data ?? null);
	// A session with zero traces yet is not worth a chip.
	const hasData = $derived(!!usage && usage.trace_count > 0);

	const cost = $derived(Number(usage?.cost_usd ?? 0));
	const costLabel = $derived(`$${cost.toFixed(cost >= 1 ? 2 : 3)}`);

	const deepLink = $derived(
		lf?.host && lf.project_id
			? `${lf.host}/project/${lf.project_id}/sessions/${id}`
			: null
	);

	const tip = $derived(
		usage
			? [
					m.sessions_langfuse_cost({ cost: costLabel }),
					m.sessions_langfuse_calls({ count: usage.trace_count }),
					`↑${compact(Number(usage.input_tokens))} ↓${compact(Number(usage.output_tokens))} ⚡${compact(Number(usage.cache_read))}`
				].join('\n')
			: ''
	);
</script>

{#if available && hasData}
	<span class="lf" title={tip}>
		{#if deepLink}
			<a href={deepLink} target="_blank" rel="noopener noreferrer" class="lf-link">
				<Text numeric size="xs" tone="muted" weight="semibold">{costLabel}</Text>
				<Text size="xs" tone="faint">{m.sessions_langfuse_calls_short({ count: usage?.trace_count ?? 0 })} ↗</Text>
			</a>
		{:else}
			<Text numeric size="xs" tone="muted" weight="semibold">{costLabel}</Text>
			<Text size="xs" tone="faint">{m.sessions_langfuse_calls_short({ count: usage?.trace_count ?? 0 })}</Text>
		{/if}
	</span>
{/if}

<style>
	.lf {
		display: inline-flex;
		gap: 0.3rem;
		align-items: baseline;
		white-space: nowrap;
	}
	.lf-link {
		display: inline-flex;
		gap: 0.3rem;
		align-items: baseline;
		text-decoration: none;
	}
	.lf-link:hover {
		text-decoration: underline;
	}
</style>
