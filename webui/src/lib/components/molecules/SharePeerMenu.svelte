<script lang="ts">
	import { Button, EmptyState, Input, Text } from '@dorsk/tsumikit';
	import { errMessage } from '$lib/api';
	import { m } from '$lib/paraglide/messages';
	import { toasts } from '$lib/toast.svelte';
	import { listPeerShares, sharePeer, unsharePeer, type PeerShare } from '$lib/rooms';
	import { shareCandidates, shareLabel, type ShareCandidate } from '$lib/peerShares';

	// Pairwise peer-addressing grants: the escape hatch for two sessions the
	// parent_id tree does not relate and that are not in the same room.
	let {
		sessionId,
		candidates = []
	}: {
		sessionId: string;
		/** The owner's other sessions, from the already-loaded list. */
		candidates?: ShareCandidate[];
	} = $props();

	let shares = $state<PeerShare[]>([]);
	let query = $state('');
	let busy = $state(false);

	const ROW = 'justify-content:flex-start;text-align:left;min-width:0;font-size:var(--fs-sm)';
	const shared = $derived(new Set(shares.map((s) => s.session_id)));
	const offerable = $derived(shareCandidates(candidates, sessionId, shared, query));

	async function load() {
		try {
			shares = await listPeerShares(sessionId);
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}

	$effect(() => {
		void sessionId;
		void load();
	});

	async function run(action: () => Promise<unknown>) {
		busy = true;
		try {
			await action();
			await load();
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = false;
		}
	}
</script>

<div class="menu">
	{#each shares as s (s.session_id)}
		<div class="row">
			<span class="who" title={s.session_id}>
				<Text size="sm">{s.name?.trim() || s.session_id}</Text>
			</span>
			<Button
				variant="ghost"
				size="sm"
				disabled={busy}
				title={m.rooms_unshare_peer()}
				aria-label={m.rooms_unshare_peer()}
				onclick={() => run(() => unsharePeer(sessionId, s.session_id))}>×</Button
			>
		</div>
	{/each}

	{#if shares.length === 0}
		<Text size="xs" tone="muted">{m.rooms_share_peer_none()}</Text>
	{/if}

	<Input
		bind:value={query}
		size="sm"
		grow
		disabled={busy}
		placeholder={m.rooms_share_peer_find()}
		aria-label={m.rooms_share_peer_find()}
		maxlength={80}
	/>

	{#each offerable as c (c.id)}
		<Button
			variant="ghost"
			size="sm"
			block
			style={ROW}
			disabled={busy}
			onclick={() => run(() => sharePeer(sessionId, c.id))}
		>
			<span class="glyph" aria-hidden="true">+</span>
			<span class="who">{shareLabel(c)}</span>
		</Button>
	{/each}

	{#if offerable.length === 0 && query.trim()}
		<EmptyState title={m.rooms_share_peer_no_match()} />
	{/if}

	<Text size="xs" tone="muted">{m.rooms_share_peer_hint()}</Text>
</div>

<style>
	.menu {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 15rem;
	}
	.row {
		display: flex;
		gap: var(--sp-1);
		align-items: center;
		min-width: 0;
	}
	.glyph {
		flex: none;
		width: 1em;
		text-align: center;
	}
	.who {
		min-width: 0;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
</style>
