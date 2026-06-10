<script lang="ts">
	import type { UserRow } from '@bindings/UserRow';
	import MachineBadge from './MachineBadge.svelte';
	import { useMachines, useTokens, useUserActions } from '$lib/queries';
	import { dateOnly, relativeTime } from '$lib/format';
	import { toasts } from '$lib/toast.svelte';

	// Inline expansion of a user row in the users table (CCT-222) — replaces
	// the old UserDetail modal sheet so nothing jumps or overlays.
	let {
		user,
		onsecret
	}: {
		user: UserRow;
		onsecret: (title: string, secret: string) => void;
	} = $props();

	const revoked = $derived(!!user.revoked_at);

	// Only mounted while expanded, so fetch eagerly.
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

	// Real enrolled daemons plus the server-managed per-user `dispatch` machine
	// (shown read-only so its badge color stays editable — CCT-251). One-shot
	// `ephemeral` worker pods stay hidden.
	const shownMachines = $derived(($machines.data ?? []).filter((m) => m.kind !== 'ephemeral'));
	const hiddenCount = $derived(($machines.data ?? []).length - shownMachines.length);

	// Preset hue swatches for the per-machine color override (CCT-222). Shown
	// in a popover anchored to the machine badge (CCT-251), not inline.
	const HUES = [0, 30, 60, 90, 120, 150, 180, 210, 240, 270, 300, 330];
	let paletteFor = $state<string | null>(null);

	function toggleDispatch() {
		const next = !user.can_dispatch;
		guard(
			actions
				.setCanDispatch(user.id, next)
				.then(() => toasts.ok(next ? 'Dispatch enabled' : 'Dispatch disabled'))
		);
	}
	function mintToken() {
		const label = prompt('Token label (optional)', '')?.trim() || null;
		guard(
			actions.mintToken(user.id, label).then((r) => onsecret(`Token — ${user.name}`, r.token))
		);
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
	function renameMachine(id: string, current: string | null, hue: number | null) {
		const displayName = prompt('Machine display name', current ?? '')?.trim() || null;
		guard(actions.updateMachine(user.id, id, displayName, hue));
	}
	function setHue(id: string, displayName: string | null, hue: number | null) {
		paletteFor = null;
		guard(actions.updateMachine(user.id, id, displayName, hue));
	}
	function revokeMachine(id: string) {
		if (confirm('Revoke this machine? Its key stops working; the daemon must re-enroll.'))
			guard(actions.revokeMachine(user.id, id));
	}
	function purgeMachine(id: string) {
		if (confirm('Permanently remove this revoked machine?')) guard(actions.purgeMachine(user.id, id));
	}
</script>

<div class="stack expand">
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

	<!-- Machines -->
	<section class="stack sec">
		<h3 class="sub-h">Machines</h3>
		{#if $machines.isLoading}<span class="spin"></span>
		{:else if !shownMachines.length}<p class="faint sm">No machines.</p>
		{:else}
			{#each shownMachines as mc (mc.id)}
				{@const system = mc.kind === 'dispatch'}
				<div class="sub-row">
					<div class="stack info">
						<span class="row badge-line">
							<!-- Clicking the badge opens the color popover (CCT-251). -->
							<button
								class="badge-btn"
								title="Badge color"
								aria-label="Badge color"
								disabled={!!mc.revoked_at}
								onclick={() => (paletteFor = paletteFor === mc.id ? null : mc.id)}
							>
								<MachineBadge name={mc.display_name || mc.name} id={mc.id} hue={mc.hue} />
							</button>
							{#if !mc.revoked_at && !system}
								<button
									class="pen"
									title="Rename machine"
									aria-label="Rename machine"
									onclick={() => renameMachine(mc.id, mc.display_name, mc.hue)}>✎</button
								>
							{/if}
							{#if system}<span class="badge">dispatch</span>{/if}
							{#if paletteFor === mc.id}
								<span class="palette-anchor">
									<span class="row palette" role="radiogroup" aria-label="Badge color">
										<button
											class="swatch auto"
											class:active={mc.hue == null}
											title="Auto (name hash)"
											aria-label="Auto color"
											onclick={() => setHue(mc.id, mc.display_name, null)}>A</button
										>
										{#each HUES as h (h)}
											<button
												class="swatch"
												class:active={mc.hue === h}
												style={`--sh:${h}`}
												title={`Hue ${h}`}
												aria-label={`Hue ${h}`}
												onclick={() => setHue(mc.id, mc.display_name, h)}
											></button>
										{/each}
									</span>
								</span>
							{/if}
						</span>
						<span class="faint sm mono">{mc.key_preview ?? '••••••••'}</span>
						<span class="faint sm"
							>{system ? 'server-managed · ' : ''}seen {relativeTime(mc.last_seen_at)}</span
						>
					</div>
					<div class="row row-wrap mini">
						{#if mc.revoked_at}
							<span class="badge badge-danger">revoked</span>
							<button class="btn btn-sm btn-danger" onclick={() => purgeMachine(mc.id)}>Purge</button>
						{:else if !system}
							<button class="btn btn-sm btn-danger" onclick={() => revokeMachine(mc.id)}>Revoke</button>
						{/if}
					</div>
				</div>
			{/each}
		{/if}
		{#if hiddenCount > 0}
			<p class="faint sm">{hiddenCount} ephemeral worker machine{hiddenCount === 1 ? '' : 's'} hidden.</p>
		{/if}
	</section>

	<!-- Tokens: many per user, all resolving to this same user. Minting lives
	     here (not on the user row) so it's clear what a "Token" is (CCT-251). -->
	<section class="stack sec">
		<div class="row sec-head">
			<h3 class="sub-h">Tokens</h3>
			<div class="spacer"></div>
			{#if !revoked}
				<button class="btn btn-sm" onclick={mintToken}>+ New token</button>
			{/if}
		</div>
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
</div>

<style>
	.expand {
		gap: var(--sp-3);
		padding: var(--sp-3) var(--sp-3) var(--sp-3) var(--sp-5);
	}
	.sec {
		gap: var(--sp-2);
	}
	.sec + .sec {
		padding-top: var(--sp-2);
		border-top: 1px solid var(--border);
	}
	.sec-head {
		gap: var(--sp-2);
	}
	.sub-h {
		font-size: var(--fs-sm);
		color: var(--text-muted);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	/* One-row layout (CCT-279 item 4): info column takes all free space and its
	   long mono key/preview truncates instead of forcing the row to wrap into a
	   squished 4-line column with a blank body. Actions stay compact on the right
	   and only wrap as a last resort on very narrow screens. */
	.sub-row {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		padding: var(--sp-2) 0;
	}
	.sub-row + .sub-row {
		border-top: 1px solid var(--border);
	}
	.info {
		flex: 1 1 auto;
		min-width: 0;
		gap: 0;
		overflow: hidden;
	}
	/* The mono key/token preview and the "seen…" line must not force overflow —
	   ellipsize them within the column. */
	.info > .mono,
	.info > .sm {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 100%;
	}
	.badge-line {
		gap: var(--sp-1);
		position: relative;
		flex-wrap: wrap;
		min-width: 0;
	}
	/* Actions hug their content on the right; never grow/shrink into the info. */
	.mini {
		flex: 0 0 auto;
	}
	.badge-btn {
		background: none;
		border: none;
		padding: 0;
		cursor: pointer;
		font: inherit;
	}
	.badge-btn:disabled {
		cursor: default;
	}
	.pen {
		flex: none;
		background: none;
		border: none;
		padding: 0 var(--sp-1);
		cursor: pointer;
		color: var(--text-muted);
		font-size: var(--fs-sm);
	}
	.pen:hover {
		color: var(--text);
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
	/* hue popover anchored to the badge (CCT-251) */
	.palette-anchor {
		position: relative;
	}
	.palette {
		position: absolute;
		top: calc(100% + 4px);
		left: 0;
		z-index: 10;
		gap: 4px;
		flex-wrap: wrap;
		width: max-content;
		max-width: 12rem;
		padding: var(--sp-2);
		background: var(--bg-elevated-2);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md, 6px);
		box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
	}
	.swatch {
		width: 1.1rem;
		height: 1.1rem;
		border-radius: 50%;
		border: 1px solid transparent;
		background: hsl(var(--sh) 55% 40%);
		padding: 0;
		cursor: pointer;
		font-size: 0;
		transition: transform 0.1s var(--ease);
	}
	.swatch:hover {
		transform: scale(1.2);
	}
	.swatch.active {
		border-color: var(--text);
		box-shadow: 0 0 0 2px var(--bg);
	}
	.swatch.auto {
		background: var(--bg-elevated);
		border: 1px dashed var(--border-strong);
		color: var(--text-muted);
		font-size: var(--fs-xs);
		line-height: 1;
	}
	.swatch.auto.active {
		border-style: solid;
		border-color: var(--text);
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
