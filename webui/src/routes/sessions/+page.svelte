<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { useSessions } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { ws } from '$lib/ws.svelte';
	import SessionCard from '$lib/components/SessionCard.svelte';
	import ConversationDrawer from '$lib/components/ConversationDrawer.svelte';
	import SpawnModal from '$lib/components/SpawnModal.svelte';
	import { drafts, LIST_DENSITY } from '$lib/drafts';
	import { notify } from '$lib/notify.svelte';

	let dense = $state(drafts.get(LIST_DENSITY) === 'compact');
	$effect(() => {
		drafts.set(LIST_DENSITY, dense ? 'compact' : 'normal');
	});

	let showArchived = $state(false);
	let openSession = $state<SessionListItem | null>(null);
	let showSpawn = $state(false);

	const sessions = useSessions(() => showArchived);
	const qc = useQueryClient();

	// live status changes from the websocket → refetch the list
	$effect(() => {
		void ws.changeTick;
		qc.invalidateQueries({ queryKey: ['sessions'] });
	});

	// Tell the notifier which drawer is open so it won't notify for it.
	$effect(() => {
		notify.openSessionId = openSession?.id ?? null;
		return () => {
			notify.openSessionId = null;
		};
	});

	// A clicked notification asks us to open its session's drawer.
	$effect(() => {
		const id = notify.pendingOpen;
		if (!id) return;
		const target = items.find((s) => s.id === id);
		if (target) {
			openSession = target;
			notify.pendingOpen = null;
		}
	});

	const items = $derived($sessions.data?.sessions ?? []);
	const ids = $derived(new Set(items.map((s) => s.id)));
	const childrenOf = $derived.by(() => {
		const map = new Map<string, SessionListItem[]>();
		for (const s of items) {
			if (s.parent_id && ids.has(s.parent_id)) {
				map.set(s.parent_id, [...(map.get(s.parent_id) ?? []), s]);
			}
		}
		return map;
	});
	const topLevel = $derived(items.filter((s) => !s.parent_id || !ids.has(s.parent_id)));

	// Classifier buckets (CCT-90), in attention-first display order. Sessions
	// that want the user's eyes float to the top; empty buckets are dropped.
	const BUCKETS: { key: SessionListItem['bucket']; label: string }[] = [
		{ key: 'blocked', label: 'Needs input' },
		{ key: 'review', label: 'Ready for review' },
		{ key: 'working', label: 'Working' },
		{ key: 'done', label: 'Completed' }
	];
	const groups = $derived(
		BUCKETS.map((b) => ({
			...b,
			sessions: topLevel.filter((s) => (s.bucket ?? 'working') === b.key)
		})).filter((g) => g.sessions.length > 0)
	);

	const pending = (id: string) => {
		void ws.changeTick; // re-derive when perms change (setPerms bumps changeTick)
		return ws.pendingCount(id);
	};

	// keep the open drawer's session object fresh as the list refetches
	const liveOpen = $derived(
		openSession ? (items.find((s) => s.id === openSession!.id) ?? openSession) : null
	);
</script>

<div class="bar row">
	<h1 class="page-title">Sessions</h1>
	<div class="spacer"></div>
	<label class="arch row">
		<input type="checkbox" bind:checked={showArchived} /> Archived
	</label>
	<button
		class="btn btn-sm"
		title="Toggle compact / detailed rows"
		onclick={() => (dense = !dense)}>{dense ? '☰ Compact' : '▤ Detailed'}</button
	>
	<button class="btn btn-primary btn-sm" onclick={() => (showSpawn = true)}>+ New</button>
</div>

{#if $sessions.isLoading}
	<div class="empty"><span class="spin"></span></div>
{:else if topLevel.length === 0}
	<div class="empty">No sessions{showArchived ? '' : ' — toggle Archived or start one'}.</div>
{:else}
	<div class="stack" class:tight={dense}>
		{#each groups as g (g.key)}
			<div class="group-header" data-bucket={g.key}>
				{g.label} <span class="count">{g.sessions.length}</span>
			</div>
			{#each g.sessions as s (s.id)}
				<SessionCard
					session={s}
					compact={dense}
					pendingCount={pending(s.id)}
					onopen={(x) => (openSession = x)}
				/>
				{#each childrenOf.get(s.id) ?? [] as c (c.id)}
					<SessionCard
						session={c}
						child
						compact={dense}
						pendingCount={pending(c.id)}
						onopen={(x) => (openSession = x)}
					/>
				{/each}
			{/each}
		{/each}
	</div>
{/if}

{#if liveOpen}
	<ConversationDrawer session={liveOpen} onclose={() => (openSession = null)} />
{/if}

{#if showSpawn}
	<SpawnModal
		onclose={() => (showSpawn = false)}
		onspawned={() => qc.invalidateQueries({ queryKey: ['sessions'] })}
	/>
{/if}

<style>
	.bar {
		margin-bottom: var(--sp-4);
		gap: var(--sp-2);
	}
	.page-title {
		font-size: var(--fs-2xl);
	}
	.arch {
		font-size: var(--fs-sm);
		color: var(--text-muted);
		gap: var(--sp-1);
	}
	.stack.tight {
		gap: var(--sp-1);
	}
	.group-header {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin-top: var(--sp-3);
		font-size: var(--fs-sm);
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--text-muted);
	}
	.group-header:first-child {
		margin-top: 0;
	}
	.group-header .count {
		font-weight: 400;
		opacity: 0.7;
	}
	.group-header[data-bucket='blocked'] {
		color: var(--warn, #d08770);
	}
	.group-header[data-bucket='review'] {
		color: var(--accent, #88c0d0);
	}
</style>
