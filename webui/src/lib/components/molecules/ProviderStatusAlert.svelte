<script lang="ts">
	import { Icon, Popover } from '@dorsk/tsumikit';
	import { useProviderStatus } from '#lib/queries/index.ts';
	import ProviderStatusPanel from '#lib/components/molecules/ProviderStatusPanel.svelte';
	import {
		degraded,
		familyLabel,
		indicatorTone,
		severityLabel,
		worstIndicator
	} from '#lib/components/molecules/provider-status.logic.ts';
	import { m } from '#lib/paraglide/messages.js';

	const q = useProviderStatus();
	const bad = $derived(degraded(q.data));
	const tone = $derived(indicatorTone(worstIndicator(q.data)));
	const label = $derived(
		m.provider_status_aria({
			states: bad.map((s) => `${familyLabel(s.family)} ${severityLabel(s.indicator)}`).join(', ')
		})
	);
</script>

{#if bad.length > 0 && tone}
	<Popover {label} placement="bottom-start" bare hitArea="compact">
		{#snippet trigger()}
			<span class="alert" data-testid="provider-status-alert" data-tone={tone} title={label}
				><Icon name="alert" size={16} {tone} /></span
			>
		{/snippet}
		<ProviderStatusPanel statuses={bad} />
	</Popover>
{/if}

<style>
	.alert {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 24px;
		height: 24px;
	}
</style>
