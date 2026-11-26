<script lang="ts">
	import { Badge, Divider, Icon, Link, Text } from '@dorsk/tsumikit';
	import type { ProviderStatus } from '#lib/queries/index.ts';
	import {
		familyLabel,
		indicatorTone,
		severityLabel
	} from '#lib/components/molecules/provider-status.logic.ts';
	import { m } from '#lib/paraglide/messages.js';

	let { statuses }: { statuses: ProviderStatus[] } = $props();
</script>

<div class="panel" data-testid="provider-status-panel">
	{#each statuses as s, i (s.family)}
		{#if i > 0}<Divider spacing="0" decorative />{/if}
		<section class="provider" data-family={s.family}>
			<div class="head">
				<Text size="sm" weight="semibold">{familyLabel(s.family)}</Text>
				<Badge size="xs" dot tone={indicatorTone(s.indicator) ?? 'muted'}
					>{severityLabel(s.indicator)}</Badge
				>
			</div>
			{#if s.description}
				<Text size="xs" tone="muted">{s.description}</Text>
			{/if}
			{#if s.components.length > 0}
				<Text size="xs" tone="faint" data-testid="provider-status-affected"
					>{m.provider_status_affected({
						components: s.components.map((c) => c.name).join(', ')
					})}</Text
				>
			{/if}
			{#each s.incidents as inc (inc.name)}
				<Text size="xs">{inc.name}</Text>
			{/each}
			<Link href={s.url} target="_blank" rel="noopener" underline="hover"
				><span class="out"
					><Text as="span" size="xs" tone="inherit">{m.provider_status_page()}</Text><Icon
						name="external"
						size={12}
					/></span
				></Link
			>
		</section>
	{/each}
</div>

<style>
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		width: 18rem;
		max-width: 100%;
		padding: var(--sp-2);
		text-align: start;
	}
	.provider {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--sp-1);
	}
	.head {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.out {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
</style>
