<script lang="ts">
	import type { UserRow } from '@bindings/UserRow';
	import Modal from './Modal.svelte';
	import { useMachines, useTokens, useUserActions, SYSTEM_MACHINE_KINDS } from '$lib/queries';
	import { dateOnly, relativeTime } from '$lib/format';
	import { toasts } from '$lib/toast.svelte';

	let {
		user,
		onclose,
		onsecret
	}: {
		user: UserRow;
		onclose: () => void;
		onsecret: (title: string, secret: string) => void;
	} = $props();

	const revoked = $derived(!!user.revoked_at);

	// The sheet is only mounted while open, so fetch eagerly.
	const machines = useMachines(
		() => user.id,
		() => true
	);
	const tokens = useTokens(
		() => user.id,
		() => true
	);
	const actions = useUserActions();
	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.err(e.message));

	// Only real enrolled daemons belong in the machines list — the per-user
	// `dispatch` machine and one-shot `ephemeral` worker pods are server-managed
	// and hidden (CCT-185).
	const realMachines = $derived(
		($machines.data ?? []).filter((m) => !SYSTEM_MACHINE_KINDS.has(m.kind))
	);
	const hiddenCount = $derived(($machines.data ?? []).length - realMachines.length);

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
	function purgeUser() {
		if (!confirm(`Permanently delete ${user.name}? This cannot be undone.`)) return;
		guard(
			actions.purgeUser(user.id).then(() => {
				toasts.ok('User deleted');
				onclose();
			})
		);
	}
	function toggleDispatch() {
		const next = !user.can_dispatch;
		guard(
			actions
				.setCanDispatch(user.id, next)
				.then(() => toasts.ok(next ? 'Dispatch enabled' : 'Dispatch disabled'))
		);
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
	function deleteToken(tokenId: string) {
		if (confirm('Delete this token? It is revoked and removed in one step.'))
			guard(actions.purgeToken(user.id, tokenId));
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

<Modal title={user.name} {onclose}>
	{#snippet body()}
		<div class="stack">
			<div class="row">
				<span class="badge" class:badge-ok={!revoked} class:badge-danger={revoked}>
					{revoked ? 'revoked' : 'active'}
				</span>
				<span class="faint sm">created {dateOnly(user.created_at)}</span>
			</div>

			{#if !revoked}
				<div class="row row-wrap">
					<button class="btn btn-sm" onclick={rename}>Rename</button>
					<button class="btn btn-sm" onclick={mint}>Mint token</button>
					<button class="btn btn-sm" onclick={rotate}>Rotate key</button>
					<button class="btn btn-sm btn-danger" onclick={revoke}>Revoke</button>
				</div>
			{:else}
				<div class="row row-wrap">
					<button class="btn btn-sm btn-danger" onclick={purgeUser}>Delete permanently</button>
				</div>
			{/if}

			<!-- Permissions (CCT-185) -->
			<section class="stack sec">
				<h3 class="sub-h">Permissions</h3>
				<div class="row perm">
					<div class="stack info">
						<span>Can dispatch</span>
						<span class="faint sm">Allow this user to dispatch k8s worker sessions.</span>
					</div>
					<button
						class="switch"
						class:on={user.can_dispatch}
						role="switch"
						aria-checked={user.can_dispatch}
						aria-label="Can dispatch"
						disabled={revoked}
						onclick={toggleDispatch}
					>
						<span class="knob"></span>
					</button>
				</div>
			</section>

			<!-- Tokens -->
			<section class="stack sec">
				<h3 class="sub-h">Tokens</h3>
				{#if $tokens.isLoading}<span class="spin"></span>
				{:else if !($tokens.data ?? []).length}<p class="faint sm">No tokens.</p>
				{:else}
					{#each $tokens.data ?? [] as t (t.id)}
						<div class="sub-row">
							<div class="stack info">
								<span class="truncate">{t.label || '(unlabeled)'}</span>
								<span class="faint sm mono">{t.token_preview ?? '••••••••'}</span>
								<span class="faint sm">
									created {dateOnly(t.created_at)}{t.expires_at
										? ` · expires ${dateOnly(t.expires_at)}`
										: ''}
								</span>
							</div>
							<div class="row row-wrap mini">
								{#if t.revoked_at}
									<span class="badge badge-danger">revoked</span>
									<button class="btn btn-sm btn-danger" onclick={() => deleteToken(t.id)}>Delete</button>
								{:else}
									<button class="btn btn-sm" onclick={() => relabelToken(t.id, t.label)}>Relabel</button>
									<button class="btn btn-sm btn-danger" onclick={() => revokeToken(t.id)}>Revoke</button>
									<button class="btn btn-sm btn-danger" onclick={() => deleteToken(t.id)}>Delete</button>
								{/if}
							</div>
						</div>
					{/each}
				{/if}
			</section>

			<!-- Machines -->
			<section class="stack sec">
				<h3 class="sub-h">Machines</h3>
				{#if $machines.isLoading}<span class="spin"></span>
				{:else if !realMachines.length}<p class="faint sm">No machines.</p>
				{:else}
					{#each realMachines as mc (mc.id)}
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
				{#if hiddenCount > 0}
					<p class="faint sm">{hiddenCount} server-managed machine{hiddenCount === 1 ? '' : 's'} hidden.</p>
				{/if}
			</section>
		</div>
	{/snippet}
</Modal>

<style>
	.sec {
		gap: var(--sp-2);
		padding-top: var(--sp-2);
		border-top: 1px solid var(--border);
	}
	.sub-h {
		font-size: var(--fs-sm);
		color: var(--text-muted);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	.sub-row {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) 0;
		flex-wrap: wrap;
	}
	.sub-row + .sub-row {
		border-top: 1px solid var(--border);
	}
	.info {
		flex: 1;
		min-width: 8rem;
		gap: 0;
	}
	.mini {
		gap: var(--sp-1);
	}
	.sm {
		font-size: var(--fs-xs);
	}
	.perm {
		gap: var(--sp-3);
	}
	/* pill toggle */
	.switch {
		flex: none;
		width: 2.75rem;
		height: 1.6rem;
		border-radius: var(--r-pill);
		border: 1px solid var(--border-strong);
		background: var(--bg-elevated-2);
		padding: 2px;
		display: flex;
		align-items: center;
		transition:
			background 0.14s var(--ease),
			border-color 0.14s var(--ease);
	}
	.switch .knob {
		width: 1.25rem;
		height: 1.25rem;
		border-radius: 50%;
		background: var(--text-muted);
		transition:
			transform 0.14s var(--ease),
			background 0.14s var(--ease);
	}
	.switch.on {
		background: var(--accent);
		border-color: var(--accent);
	}
	.switch.on .knob {
		transform: translateX(1.15rem);
		background: var(--text-on-accent);
	}
	.switch:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}
</style>
