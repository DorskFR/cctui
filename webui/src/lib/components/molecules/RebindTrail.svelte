<script lang="ts">
	import { Tooltip } from '@dorsk/tsumikit';
	import { useSessionRebinds } from '$lib/queries';
	import { m } from '$lib/paraglide/messages';

	// A session that changed accounts mid-run says so, here, next to the account
	// badge — a small arrow whose tooltip lists every move with its cause.
	//
	// This is the point of pools, not a decoration. Account movement used to be
	// invisible: it was found out after the fact, in a bill, which is what made
	// an otherwise useful feature feel like a betrayal. A session that never
	// moved renders nothing at all, so the marker means exactly one thing.
	let { sessionId, enabled = true }: { sessionId: string; enabled?: boolean } = $props();

	const rebinds = useSessionRebinds(
		() => sessionId,
		() => enabled && !!sessionId,
	);
	const moves = $derived(rebinds.data ?? []);
	const reasonLabel = (reason: string) =>
		reason === 'pool' ? m.sessions_rebind_reason_pool() : m.sessions_rebind_reason_redirect();
	// Oldest first in the tooltip: the trail reads as the path the session
	// actually walked, not as a reverse-chronological log.
	const trail = $derived(
		[...moves]
			.reverse()
			.map((r) => `${r.from_account} → ${r.to_account} (${reasonLabel(r.reason)})`)
			.join('\n'),
	);
</script>

{#if moves.length > 0}
	<Tooltip text={`${m.sessions_rebinds_tip()}\n${trail}`}>
		{#snippet trigger()}
			<span class="trail" aria-label={m.sessions_rebinds_tip()}>
				<!-- lucide arrow-right-left, sized in em so it tracks the font-scale picker. -->
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
					aria-hidden="true"
				>
					<path d="m16 3 4 4-4 4" /><path d="M20 7H4" />
					<path d="m8 21-4-4 4-4" /><path d="M4 17h16" />
				</svg>
				{#if moves.length > 1}
					<span class="count">{moves.length}</span>
				{/if}
			</span>
		{/snippet}
	</Tooltip>
{/if}

<style>
	.trail {
		display: inline-flex;
		align-items: center;
		gap: 0.15em;
		flex: none;
		color: var(--c-muted, currentColor);
		cursor: default;
	}
	.trail svg {
		width: 1em;
		height: 1em;
	}
	.count {
		font-size: var(--fs-xs);
		line-height: 1;
	}
</style>
