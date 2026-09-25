<script lang="ts">
	import { Dot } from '@dorsk/tsumikit';
	import MachineBadge from '$lib/components/molecules/MachineBadge.svelte';
	import SessionSectionHeader from '$lib/components/molecules/SessionSectionHeader.svelte';
	import { m } from '$lib/paraglide/messages';
	import type { SessionsPage } from './sessionsPage.svelte';

	// One header for every section: label, live count, sort menu, an eye toggle
	// that collapses the rows, and (where `archiveIds` is given) a bulk archive
	// behind the shared confirm dialog.
	let {
		sp,
		key,
		label,
		count,
		hue = null,
		bucket = null,
		machine = null,
		liveness = null,
		archiveIds
	}: {
		sp: SessionsPage;
		key: string;
		label: string;
		count: number;
		hue?: number | null;
		bucket?: string | null;
		machine?: string | null;
		liveness?: 'online' | 'stale' | 'offline' | null;
		archiveIds?: string[];
	} = $props();

	function bucketColor(key: string | null | undefined): string {
		switch (key) {
			case 'blocked':
				return 'var(--warn)';
			case 'review':
				return 'var(--accent)';
			case 'working':
				return 'var(--ok)';
			case 'dispatched':
				return 'var(--info)';
			default:
				return 'var(--text-faint)';
		}
	}
</script>

<SessionSectionHeader
	{label}
	title={machine ? '' : label}
	{count}
	hue={machine || hue == null ? undefined : hue}
	lead={headerLead}
	sort={sp.sortState.sort}
	sortDir={sp.sortState.sortDir}
	onsort={sp.selectSort}
	hidden={sp.hiddenSections.has(key)}
	ontogglehidden={() => sp.toggleSection(key)}
	onarchive={archiveIds ? () => sp.archiveSection(label, archiveIds) : undefined}
	archiving={sp.archiving}
/>
{#snippet headerLead()}
	{#if machine}
		<MachineBadge name={machine} id={machine} {hue} mono />
		{#if liveness}
			<span class="liveness" class:online={liveness === 'online'}>
				<Dot status={liveness === 'online' ? 'active' : liveness === 'stale' ? 'stale' : 'dead'} />
				{liveness === 'online'
					? m.sessions_machine_online()
					: liveness === 'stale'
						? m.sessions_machine_stale()
						: m.sessions_machine_offline()}
			</span>
		{/if}
	{:else if bucket}
		<Dot color={bucketColor(bucket)} />
	{/if}
{/snippet}

<style>
	.liveness {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.liveness.online {
		color: var(--ok);
	}
</style>
