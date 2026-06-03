<script lang="ts">
	import type { UserRow } from '@bindings/UserRow';
	import { useUsers, useUserActions } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { dateOnly } from '$lib/format';
	import UserDetail from '$lib/components/UserDetail.svelte';
	import SecretReveal from '$lib/components/SecretReveal.svelte';

	const users = useUsers();
	const actions = useUserActions();

	let secret = $state<{ title: string; value: string } | null>(null);
	let selectedId = $state<string | null>(null);
	let filter = $state('');

	function showSecret(title: string, value: string) {
		secret = { title, value };
	}

	async function createUser() {
		const name = prompt('New user name')?.trim();
		if (!name) return;
		try {
			const r = await actions.create(name);
			showSecret(`Key — ${r.name}`, r.key);
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	// Active first, then revoked; within each, by creation order (stable).
	const sorted = $derived(
		[...($users.data ?? [])].sort((a, b) => Number(!!a.revoked_at) - Number(!!b.revoked_at))
	);
	const shown = $derived(
		filter.trim()
			? sorted.filter((u) => u.name.toLowerCase().includes(filter.trim().toLowerCase()))
			: sorted
	);
	// Re-resolve the selected user from fresh query data so the open sheet
	// reflects edits (rename, dispatch toggle) without being reopened.
	const selected = $derived<UserRow | null>(
		selectedId ? (($users.data ?? []).find((u) => u.id === selectedId) ?? null) : null
	);
</script>

<div class="bar row">
	<h1 class="page-title">Users</h1>
	<div class="spacer"></div>
	<button class="btn btn-primary btn-sm" onclick={createUser}>+ New user</button>
</div>

{#if ($users.data ?? []).length > 6}
	<input class="input filter" placeholder="Filter users…" bind:value={filter} />
{/if}

{#if $users.isLoading}
	<div class="empty"><span class="spin"></span></div>
{:else if shown.length === 0}
	<div class="empty">{filter.trim() ? 'No matching users.' : 'No users yet.'}</div>
{:else}
	<div class="stack">
		{#each shown as u (u.id)}
			<button class="card card-tap row item" onclick={() => (selectedId = u.id)}>
				<div class="stack who">
					<span class="name truncate">{u.name}</span>
					<span class="faint sm">created {dateOnly(u.created_at)}</span>
				</div>
				<div class="spacer"></div>
				<div class="row row-wrap chips">
					{#if u.revoked_at}
						<span class="badge badge-danger">revoked</span>
					{:else}
						<span class="badge badge-ok">active</span>
						{#if !u.can_dispatch}
							<span class="badge badge-warn">no dispatch</span>
						{/if}
					{/if}
				</div>
				<span class="faint chev">›</span>
			</button>
		{/each}
	</div>
{/if}

{#if selected}
	<UserDetail user={selected} onclose={() => (selectedId = null)} onsecret={showSecret} />
{/if}

{#if secret}
	<SecretReveal title={secret.title} secret={secret.value} onclose={() => (secret = null)} />
{/if}

<style>
	.bar {
		margin-bottom: var(--sp-4);
	}
	.page-title {
		font-size: var(--fs-2xl);
	}
	.filter {
		margin-bottom: var(--sp-3);
	}
	.item {
		width: 100%;
		text-align: left;
		gap: var(--sp-3);
	}
	.who {
		gap: 0;
		min-width: 0;
	}
	.name {
		font-weight: var(--fw-semibold);
	}
	.sm {
		font-size: var(--fs-xs);
	}
	.chips {
		gap: var(--sp-1);
		flex: none;
	}
	.chev {
		flex: none;
		font-size: var(--fs-lg);
	}
</style>
