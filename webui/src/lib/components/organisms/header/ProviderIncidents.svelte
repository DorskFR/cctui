<script lang="ts">
	import { Text } from '@dorsk/tsumikit';
	import type { ProviderStatus } from '$lib/queries';
	import { familyLabel } from '$lib/components/molecules/provider-status.logic';
	import { m } from '$lib/paraglide/messages';

	let { incidents }: { incidents: ProviderStatus[] } = $props();

	const severity = (i: string) =>
		i === 'critical'
			? m.provider_status_critical()
			: i === 'major'
				? m.provider_status_major()
				: m.provider_status_minor();
</script>

{#each incidents as s (s.family)}
	<div class="row">
		<Text size="sm" weight="semibold"
			>{m.provider_status_heading({ provider: familyLabel(s.family), severity: severity(s.indicator) })}</Text
		>
		{#if s.description}
			<Text size="xs" tone="faint">{s.description}</Text>
		{/if}
		{#if s.components.length > 0}
			<Text size="xs">
				{m.provider_status_affected({ components: s.components.map((c) => c.name).join(', ') })}
			</Text>
		{/if}
		{#each s.incidents as inc (inc.name)}
			<Text size="xs" tone="faint">{inc.name}</Text>
		{/each}
		<a class="link" href={s.url} target="_blank" rel="noopener">{m.provider_status_page()}</a>
	</div>
{/each}

<style>
	.row {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.link {
		width: fit-content;
		color: var(--accent);
		font-size: var(--fs-xs);
		text-decoration: none;
	}
	.link:hover {
		text-decoration: underline;
	}
</style>
