<script lang="ts">
	import { useCapabilities, useSessionLangfuse } from '$lib/queries';
	import { formatStat } from '$lib/statText';
	import { hasLangfuseCost, langfuseCostLabel } from '$lib/spend';
	import { Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	// Keys live only on the server: the figures are proxied through
	// `/sessions/{id}/langfuse`, the browser never talks to Langfuse.
	let {
		id,
		enabled = true
	}: {
		id: string;
		/** Gate the fetch (e.g. only while the drawer is open). */
		enabled?: boolean;
	} = $props();

	const caps = useCapabilities();
	const lf = $derived(caps.data?.langfuse);
	const available = $derived(!!lf?.available);

	const q = useSessionLangfuse(
		() => id,
		() => enabled && available
	);

	const usage = $derived(q.data ?? null);
	const hasData = $derived(hasLangfuseCost(usage));

	const costLabel = $derived(langfuseCostLabel(Number(usage?.cost_usd ?? 0)));

	const deepLink = $derived(
		lf?.public_host && lf.project_id
			? `${lf.public_host}/project/${lf.project_id}/sessions/${id}`
			: null
	);

	const tip = $derived(
		usage
			? [
					m.sessions_langfuse_cost({ cost: costLabel }),
					m.sessions_langfuse_calls({ count: usage.trace_count }),
					[
						formatStat(Number(usage.input_tokens), 'input'),
						formatStat(Number(usage.output_tokens), 'output'),
						formatStat(Number(usage.cache_read), 'cache')
					].join(' · ')
				].join('\n')
			: ''
	);
</script>

{#if available && hasData}
	<span class="lf" title={tip}>
		{#if deepLink}
			<a href={deepLink} target="_blank" rel="noopener noreferrer" class="lf-link">
				<Text numeric size="xs" tone="muted">{m.chrome_cost_langfuse({ cost: costLabel })} ↗</Text>
			</a>
		{:else}
			<Text numeric size="xs" tone="muted">{m.chrome_cost_langfuse({ cost: costLabel })}</Text>
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
