<script lang="ts">
	import { Popover, Text } from '@dorsk/tsumikit';
	import { useProviderStatus } from '$lib/queries';
	import {
		degraded,
		familyLabel,
		indicatorTone,
		worstIndicator
	} from '$lib/components/molecules/provider-status.logic';
	import { m } from '$lib/paraglide/messages';

	const q = useProviderStatus();
	const bad = $derived(degraded(q.data));
	const worst = $derived(worstIndicator(q.data));
	const tone = $derived(indicatorTone(worst));

	const severity = (i: string) =>
		i === 'critical'
			? m.provider_status_critical()
			: i === 'major'
				? m.provider_status_major()
				: m.provider_status_minor();
</script>

{#if bad.length > 0 && tone}
	<Popover
		label={m.provider_status_aria({ count: bad.length })}
		placement="bottom-end"
		bare
		hitArea="compact"
	>
		{#snippet trigger()}
			<span
				class="ind"
				data-tone={tone}
				title={bad.map((s) => `${familyLabel(s.family)} · ${severity(s.indicator)}`).join('\n')}
			>
				<span class="dot"></span>
				<span class="who">{bad.map((s) => familyLabel(s.family)).join(', ')}</span>
			</span>
		{/snippet}
		<div class="panel">
			{#each bad as s (s.family)}
				<div class="row">
					<Text size="sm" weight="semibold"
						>{m.provider_status_heading({
							provider: familyLabel(s.family),
							severity: severity(s.indicator)
						})}</Text
					>
					{#if s.description}
						<Text size="xs" tone="faint">{s.description}</Text>
					{/if}
					{#if s.components.length > 0}
						<Text size="xs">
							{m.provider_status_affected({
								components: s.components.map((c) => c.name).join(', ')
							})}
						</Text>
					{/if}
					{#each s.incidents as inc (inc.name)}
						<Text size="xs" tone="faint">{inc.name}</Text>
					{/each}
					<a class="link" href={s.url} target="_blank" rel="noopener">
						<Text as="span" size="xs" tone="accent">{m.provider_status_page()}</Text>
					</a>
				</div>
			{/each}
		</div>
	</Popover>
{/if}

<style>
	/* Lives in the px-pinned header: every length is px, never rem. */
	.ind {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		height: 24px;
		padding: 0 6px;
		border: 1px solid var(--border);
		border-radius: 4px;
		font-size: 11px;
		line-height: 1;
		white-space: nowrap;
	}
	.dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		flex: none;
	}
	.ind[data-tone='warn'] .dot {
		background: var(--warn);
	}
	.ind[data-tone='danger'] .dot {
		background: var(--danger);
		box-shadow: 0 0 6px var(--danger);
	}
	.ind[data-tone='warn'] .who {
		color: var(--warn);
	}
	.ind[data-tone='danger'] .who {
		color: var(--danger);
	}
	/* The header has no room for names once the nav is served; the dot alone
	   still carries the signal and the popover carries the detail. */
	@media (max-width: 1023px) {
		.who {
			display: none;
		}
	}
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		width: 18rem;
		max-width: 100%;
	}
	.row {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.link {
		text-decoration: none;
		width: fit-content;
	}
	.link:hover {
		text-decoration: underline;
	}
</style>
