<script lang="ts">
	import type { UserRow } from '@bindings/UserRow';
	import { useMachines, useTokens, useUserActions } from '$lib/queries';
	import { dateOnly, relativeTime } from '$lib/format';
	import { toasts } from '$lib/toast.svelte';

	let {
		user,
		onsecret
	}: { user: UserRow; onsecret: (title: string, secret: string) => void } = $props();

	let expanded = $state(false);
	const revoked = $derived(!!user.revoked_at);

	const machines = useMachines(
		() => user.id,
		() => expanded
	);
	const tokens = useTokens(
		() => user.id,
		() => expanded
	);
	const actions = useUserActions();
	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.err(e.message));

	function rename() {
		const name = prompt('New user name', user.name)?.trim();
		if (name) guard(actions.rename(user.id, name).then(() => toasts.ok('Renamed')));
	}
	function rotate() {
		if (!confirm(`Rotate key for ${user.name}? The old key stops working.`)) return;
		guard(actions.rotate(user.id).then((r) => onsecret(`New key — ${user.name}`, r.key)));
	}
	function revoke() {
		if (!confirm(`Revoke ${user.name}? All their machine keys are invalidated.`)) return;
		guard(actions.revoke(user.id).then(() => toasts.ok('Revoked')));
	}
	function mint() {
		const label = prompt('Token label (optional)', '')?.trim() || null;
		guard(actions.mintToken(user.id, label).then((r) => onsecret(`Token — ${user.name}`, r.token)));
	}
	function relabelToken(tokenId: string, current: string | null) {
		const label = prompt('Token label', current ?? '')?.trim() || null;
		guard(actions.relabelToken(user.id, tokenId, label));
	}
	function revokeToken(tokenId: string) {
		if (confirm('Revoke this token?')) guard(actions.revokeToken(user.id, tokenId));
	}
	function rotateMachine(id: string) {
		if (!confirm('Rotate this machine key?')) return;
		guard(actions.rotateMachine(user.id, id).then((r) => onsecret('New machine key', r.key)));
	}
	function renameMachine(id: string, current: string | null) {
		const displayName = prompt('Machine display name', current ?? '')?.trim() || null;
		guard(actions.renameMachine(user.id, id, displayName));
	}
	function revokeMachine(id: string) {
		if (confirm('Revoke this machine?')) guard(actions.revokeMachine(user.id, id));
	}
	function purgeMachine(id: string) {
		if (confirm('Permanently remove this revoked machine?')) guard(actions.purgeMachine(user.id, id));
	}
</script>

<div class="card stack uc">
	<div class="row head">
		<button class="exp btn btn-ghost btn-icon" onclick={() => (expanded = !expanded)} aria-label="Expand">
			{expanded ? '▾' : '▸'}
		</button>
		<div class="stack who">
			<span class="name truncate">{user.name}</span>
			<span class="faint sm">created {dateOnly(user.created_at)}</span>
		</div>
		<div class="spacer"></div>
		<span class="badge" class:badge-ok={!revoked} class:badge-danger={revoked}>
			{revoked ? 'revoked' : 'active'}
		</span>
	</div>

	{#if !revoked}
		<div class="row row-wrap acts">
			<button class="btn btn-sm" onclick={rename}>Rename</button>
			<button class="btn btn-sm" onclick={mint}>Mint token</button>
			<button class="btn btn-sm" onclick={rotate}>Rotate key</button>
			<button class="btn btn-sm btn-danger" onclick={revoke}>Revoke</button>
		</div>
	{/if}

	{#if expanded}
		<div class="divider"></div>
		<section>
			<h3 class="sub-h">Machines</h3>
			{#if $machines.isLoading}<span class="spin"></span>
			{:else if !($machines.data ?? []).length}<p class="faint sm">No machines.</p>
			{:else}
				{#each $machines.data ?? [] as mc (mc.id)}
					<div class="sub-row">
						<div class="stack info">
							<span class="truncate">{mc.display_name || mc.name}</span>
							<span class="faint sm">seen {relativeTime(mc.last_seen_at)}</span>
						</div>
						<div class="row row-wrap mini">
							{#if mc.revoked_at}
								<span class="badge badge-danger">revoked</span>
								<button class="btn btn-sm btn-danger" onclick={() => purgeMachine(mc.id)}>Purge</button>
							{:else}
								<button class="btn btn-sm" onclick={() => renameMachine(mc.id, mc.display_name)}>Rename</button>
								<button class="btn btn-sm" onclick={() => rotateMachine(mc.id)}>Rotate</button>
								<button class="btn btn-sm btn-danger" onclick={() => revokeMachine(mc.id)}>Revoke</button>
							{/if}
						</div>
					</div>
				{/each}
			{/if}
		</section>

		<section>
			<h3 class="sub-h">Tokens</h3>
			{#if $tokens.isLoading}<span class="spin"></span>
			{:else if !($tokens.data ?? []).length}<p class="faint sm">No tokens.</p>
			{:else}
				{#each $tokens.data ?? [] as t (t.id)}
					<div class="sub-row">
						<div class="stack info">
							<span class="truncate">{t.label || '(unlabeled)'}</span>
							<span class="faint sm">
								created {dateOnly(t.created_at)}{t.expires_at ? ` · expires ${dateOnly(t.expires_at)}` : ''}
							</span>
						</div>
						<div class="row mini">
							{#if t.revoked_at}
								<span class="badge badge-danger">revoked</span>
							{:else}
								<button class="btn btn-sm" onclick={() => relabelToken(t.id, t.label)}>Relabel</button>
								<button class="btn btn-sm btn-danger" onclick={() => revokeToken(t.id)}>Revoke</button>
							{/if}
						</div>
					</div>
				{/each}
			{/if}
		</section>
	{/if}
</div>

<style>
	.uc {
		gap: var(--sp-3);
	}
	.head {
		gap: var(--sp-2);
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
	.exp {
		flex: none;
	}
	.sub-h {
		font-size: var(--fs-sm);
		color: var(--text-muted);
		margin-bottom: var(--sp-2);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	.sub-row {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) 0;
		border-top: 1px solid var(--border);
		flex-wrap: wrap;
	}
	.sub-row .info {
		flex: 1;
		min-width: 8rem;
		gap: 0;
	}
	.mini {
		gap: var(--sp-1);
		flex-wrap: wrap;
	}
</style>
