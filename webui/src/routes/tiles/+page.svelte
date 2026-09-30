<script lang="ts">
	import { page } from '$app/state';
	import { replaceState } from '$app/navigation';
	import { untrack } from 'svelte';
	import { EmptyState } from '@dorsk/tsumikit';
	import { endpoints, qk, useSessions } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { toasts } from '$lib/toast.svelte';
	import { settings } from '$lib/settings.svelte';
	import { drafts } from '$lib/drafts';
	import { awaitSessionRow } from '$lib/awaitSessionRow';
	import { buildSessionSearchSchema, contextForField } from '$lib/searchSchema';
	import { pickerMatches } from '$lib/tilesPicker';
	import { TilesWorkspace } from '$lib/tiles.svelte';
	import { tileLayout, type SplitDirection } from '$lib/tiles';
	import SpawnModal from '$lib/components/organisms/SpawnModal.svelte';
	import TilesToolbar from './TilesToolbar.svelte';
	import TilesGrid from './TilesGrid.svelte';
	import { m } from '$lib/paraglide/messages';

	const sessions = useSessions(() => false);
	const qc = useQueryClient();
	const rows = $derived(sessions.data?.sessions ?? []);

	const tiles = new TilesWorkspace({
		store: { get: (k) => drafts.get(k), set: (k, v) => drafts.set(k, v) },
		maxTiles: () => settings.state.tiles.maxTiles,
		onOverCap: (max) => toasts.info(m.tiles_over_cap({ max })),
		writeUrl: (ids) => {
			const url = new URL(page.url);
			if (ids.length) url.searchParams.set('s', ids.join(','));
			else url.searchParams.delete('s');
			replaceState(url, page.state);
		}
	});
	// A shared `/tiles?s=…` link wins over the persisted set; after that the
	// workspace owns the URL.
	$effect(() => {
		untrack(() => tiles.hydrate(page.url.searchParams.get('s')));
	});

	// Below the drawer's own breakpoint the grid shows one pane at a time; the
	// tab strip drives the same `focused` state the grid outlines.
	let narrow = $state(false);
	$effect(() => {
		const mq = window.matchMedia('(max-width: 959px)');
		const sync = () => (narrow = mq.matches);
		sync();
		mq.addEventListener('change', sync);
		return () => mq.removeEventListener('change', sync);
	});

	const shownIds = $derived(
		narrow ? [tiles.focused ?? tiles.ids[0]].filter((id): id is string => !!id) : tiles.visible
	);
	const layout = $derived(tileLayout(shownIds.length, settings.state.tiles.splitDirection));
	// A just-spawned session has no row yet, so its slot stays null until
	// `awaitSessionRow` fills it.
	const shownSessions = $derived(shownIds.map((id) => rows.find((s) => s.id === id) ?? null));

	let rawPick = $state('');
	const pickerSchema = buildSessionSearchSchema((field, q) =>
		endpoints.searchFieldValues(field, q, contextForField(rawPick, pickerSchema, field))
	);
	const candidates = $derived(
		rows.filter((s) => !tiles.ids.includes(s.id) && pickerMatches(s, rawPick, pickerSchema))
	);

	let showSpawn = $state(false);

	async function adopt(id: string) {
		if (!tiles.add(id)) return;
		if (rows.some((s) => s.id === id)) return;
		const row = await awaitSessionRow(id, {
			loaded: () => rows,
			fetchOne: (sid) => endpoints.session(sid),
			refetchLive: () => qc.invalidateQueries({ queryKey: qk.sessions(false) })
		});
		if (!row) {
			toasts.error(m.sessions_toast_fork_slow());
			tiles.remove(id);
		}
	}

	function onSpawned(sessionId: string | null) {
		void qc.invalidateQueries({ queryKey: qk.sessionsAll });
		if (sessionId) void adopt(sessionId);
	}

	function setSplit(d: SplitDirection) {
		settings.setTiles({ splitDirection: d });
	}
</script>

<svelte:head><title>{m.nav_tiles()}</title></svelte:head>

<div class="tiles-page">
	<TilesToolbar
		schema={pickerSchema}
		bind:rawQuery={rawPick}
		{candidates}
		count={tiles.ids.length}
		max={settings.state.tiles.maxTiles}
		splitDirection={settings.state.tiles.splitDirection}
		onadd={(id) => void adopt(id)}
		onnew={() => (showSpawn = true)}
		onsplit={setSplit}
	/>

	{#if tiles.ids.length === 0}
		<div class="empty">
			<EmptyState
				icon="grid"
				title={m.tiles_empty_title()}
				description={m.tiles_empty_body()}
				actionLabel={m.tiles_new()}
				onAction={() => (showSpawn = true)}
			/>
		</div>
	{:else}
		<div class="tabs" role="tablist" aria-label={m.nav_tiles()}>
			{#each tiles.ids as id (id)}
				{@const s = rows.find((r) => r.id === id)}
				<button
					type="button"
					role="tab"
					aria-selected={tiles.focused === id}
					class:on={tiles.focused === id}
					onclick={() => tiles.focus(id)}
				>
					{s?.name || id.slice(0, 8)}
				</button>
			{/each}
		</div>
		<TilesGrid
			{tiles}
			ids={shownIds}
			{layout}
			sessions={shownSessions}
			onnavigate={(from, to) => {
				tiles.remove(from);
				void adopt(to);
			}}
		/>
	{/if}
</div>

{#if showSpawn}
	<SpawnModal onclose={() => (showSpawn = false)} onspawned={onSpawned} />
{/if}

<style>
	.tiles-page {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
	}
	.empty {
		flex: 1;
		display: grid;
		place-items: center;
	}
	.tabs {
		display: none;
	}
	@media (max-width: 959px) {
		.tabs {
			display: flex;
			gap: 1px;
			overflow-x: auto;
			background: var(--border-strong);
			flex: none;
		}
		.tabs button {
			flex: 1 0 auto;
			padding: var(--sp-2) var(--sp-3);
			border: 0;
			background: var(--bg-elevated);
			color: var(--text-muted);
			font: inherit;
			white-space: nowrap;
			cursor: pointer;
		}
		.tabs button.on {
			background: var(--bg);
			color: var(--text);
		}
	}
</style>
