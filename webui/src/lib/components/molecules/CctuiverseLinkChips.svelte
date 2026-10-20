<script lang="ts">
	import { Dot, Popover, Text } from '@dorsk/tsumikit';
	import { listLinks, openLinks, type InviteTarget, type LinkView } from '$lib/cctuiverse';
	import { cctuiverseConfig } from '$lib/cctuiverseConfig.svelte';
	import { ws } from '$lib/ws.svelte';
	import { m } from '$lib/paraglide/messages';
	import CctuiverseLinkPanel from './CctuiverseLinkPanel.svelte';

	let { target }: { target: InviteTarget } = $props();

	let links = $state<LinkView[]>([]);

	const key = $derived('session' in target ? target.session : target.room);
	const shown = $derived(openLinks(links));

	async function load(t: InviteTarget) {
		try {
			links = await listLinks(t);
		} catch {
			links = [];
		}
	}

	$effect(() => {
		if (!cctuiverseConfig.enabled) return;
		const t = target;
		void key;
		void load(t);
		return ws.onCctuiverseChanged((ev) => {
			if ('session' in t ? ev.session_id === t.session : ev.room_id === t.room) void load(t);
		});
	});

	function replace(next: LinkView) {
		links = links.map((l) => (l.id === next.id ? next : l));
	}
</script>

{#each shown as link (link.id)}
	<Popover
		label={m.cctuiverse_chip_label({ peer: link.peer_label ?? m.cctuiverse_invite_pending() })}
		placement="bottom-start"
		size="sm"
		pill
		count={Number(link.held_count) + Number(link.review_count) || undefined}
	>
		{#snippet trigger()}
			<span class="chip">
				<Dot status={link.state === 'active' ? 'active' : 'stale'} />
				<Text size="xs" truncate>{link.peer_label ?? m.cctuiverse_invite_pending()}</Text>
			</span>
		{/snippet}
		<CctuiverseLinkPanel {link} onchange={replace} />
	</Popover>
{/each}

<style>
	.chip {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		max-width: 12rem;
		min-width: 0;
	}
</style>
