<script lang="ts">
	import { useUsers, useUserActions } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import UserCard from '$lib/components/UserCard.svelte';
	import SecretReveal from '$lib/components/SecretReveal.svelte';

	const users = useUsers();
	const actions = useUserActions();

	let secret = $state<{ title: string; value: string } | null>(null);
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

	const sorted = $derived(
		[...($users.data ?? [])].sort((a, b) => Number(!!a.revoked_at) - Number(!!b.revoked_at))
	);
</script>

<div class="bar row">
	<h1 class="page-title">Users</h1>
	<div class="spacer"></div>
	<button class="btn btn-primary btn-sm" onclick={createUser}>+ New user</button>
</div>

{#if $users.isLoading}
	<div class="empty"><span class="spin"></span></div>
{:else if sorted.length === 0}
	<div class="empty">No users yet.</div>
{:else}
	<div class="stack">
		{#each sorted as u (u.id)}
			<UserCard user={u} onsecret={showSecret} />
		{/each}
	</div>
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
</style>
