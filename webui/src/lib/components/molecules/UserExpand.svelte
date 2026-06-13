<script lang="ts">
	import type { UserRow } from '@bindings/UserRow';
	import MachineBadge from '$lib/components/molecules/MachineBadge.svelte';
	import Badge from '$lib/components/atoms/Badge.svelte';
	import Button from '$lib/components/atoms/Button.svelte';
	import Switch from '$lib/components/atoms/Switch.svelte';
	import Heading from '$lib/components/atoms/Heading.svelte';
	import Text from '$lib/components/atoms/Text.svelte';
	import IconButton from '$lib/components/molecules/IconButton.svelte';
	import ColorPicker from '$lib/components/molecules/ColorPicker.svelte';
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
		<Heading level={3} size="sm" tone="muted" class="sub-h">Permissions</Heading>
		<div class="row perm">
			<div class="stack info">
				<Text>Can dispatch</Text>
				<Text size="xs" tone="faint">Allow this user to dispatch k8s worker sessions.</Text>
			</div>
			<Switch
				checked={user.can_dispatch}
				label="Can dispatch"
				disabled={revoked}
				onclick={toggleDispatch}
			/>
		</div>
	</section>

	<!-- Machines -->
	<section class="stack sec">
		<Heading level={3} size="sm" tone="muted" class="sub-h">Machines</Heading>
		{#if $machines.isLoading}<span class="spin"></span>
		{:else if !shownMachines.length}<Text as="p" size="xs" tone="faint">No machines.</Text>
		{:else}
			{#each shownMachines as mc (mc.id)}
				{@const system = mc.kind === 'dispatch'}
				<div class="sub-row">
					<!-- One compact horizontal line per machine (CCT-301 #3): badge,
					     key preview and "seen" sit inline instead of stacking into a
					     tall 3-line block. Wraps only when too narrow. -->
					<div class="info info-inline">
						<span class="row badge-line">
							<!-- Clicking the badge opens the color popover (CCT-251). -->
							<ColorPicker
								value={mc.hue}
								hues={HUES}
								label="Badge color"
								disabled={!!mc.revoked_at}
								onchange={(h) => setHue(mc.id, mc.display_name, h)}
							>
								{#snippet trigger()}
									<MachineBadge name={mc.display_name || mc.name} id={mc.id} hue={mc.hue} />
								{/snippet}
							</ColorPicker>
							{#if !mc.revoked_at && !system}
								<IconButton
									inline
									icon="edit"
									size={14}
									title="Rename machine"
									label="Rename machine"
									onclick={() => renameMachine(mc.id, mc.display_name, mc.hue)}
								/>
							{/if}
							{#if system}<Badge>dispatch</Badge>{/if}
						</span>
						<Text size="xs" tone="faint" variant="code" truncate class="mono"
							>{mc.key_preview ?? '••••••••'}</Text
						>
						<Text size="xs" tone="faint" truncate
							>{system ? 'server-managed · ' : ''}seen {relativeTime(mc.last_seen_at)}</Text
						>
					</div>
					<div class="row row-wrap mini">
						{#if mc.revoked_at}
							<Badge tone="danger">revoked</Badge>
							<Button size="sm" variant="danger" onclick={() => purgeMachine(mc.id)}>Purge</Button>
						{:else if !system}
							<Button size="sm" variant="danger" onclick={() => revokeMachine(mc.id)}>Revoke</Button>
						{/if}
					</div>
				</div>
			{/each}
		{/if}
		{#if hiddenCount > 0}
			<Text as="p" size="xs" tone="faint"
				>{hiddenCount} ephemeral worker machine{hiddenCount === 1 ? '' : 's'} hidden.</Text
			>
		{/if}
	</section>

	<!-- Tokens: many per user, all resolving to this same user. Minting lives
	     here (not on the user row) so it's clear what a "Token" is (CCT-251). -->
	<section class="stack sec">
		<div class="row sec-head">
			<Heading level={3} size="sm" tone="muted" class="sub-h">Tokens</Heading>
			<div class="spacer"></div>
			{#if !revoked}
				<Button size="sm" onclick={mintToken}>+ New token</Button>
			{/if}
		</div>
		{#if $tokens.isLoading}<span class="spin"></span>
		{:else if !($tokens.data ?? []).length}<Text as="p" size="xs" tone="faint">No tokens.</Text>
		{:else}
			{#each $tokens.data ?? [] as t (t.id)}
				<div class="sub-row">
					<div class="stack info">
						<Text truncate>{t.label || '(unlabeled)'}</Text>
						<Text size="xs" tone="faint" variant="code" truncate class="mono"
							>{t.token_preview ?? '••••••••'}</Text
						>
						<Text size="xs" tone="faint" truncate>
							created {dateOnly(t.created_at)}{t.expires_at
								? ` · expires ${dateOnly(t.expires_at)}`
								: ''}
						</Text>
					</div>
					<div class="row row-wrap mini">
						{#if t.revoked_at}
							<Badge tone="danger">revoked</Badge>
							<Button size="sm" variant="danger" onclick={() => deleteToken(t.id)}>Delete</Button>
						{:else}
							<Button size="sm" onclick={() => relabelToken(t.id, t.label)}>Relabel</Button>
							<Button size="sm" variant="danger" onclick={() => revokeToken(t.id)}>Revoke</Button>
							<Button size="sm" variant="danger" onclick={() => deleteToken(t.id)}>Delete</Button>
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
	/* Heading owns the size/colour; this selector targets the element Heading
	   renders (so it must be :global) to add only the section-label chrome. */
	:global(.sub-h) {
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
	/* Machine rows (CCT-301 #3): lay the badge, key preview and "seen" out on a
	   single horizontal line so each machine is one compact row, not a tall
	   3-line stack. Wraps to a second line only when the column is too narrow. */
	.info-inline {
		display: flex;
		flex-direction: row;
		align-items: center;
		flex-wrap: wrap;
		column-gap: var(--sp-3);
		row-gap: var(--sp-1);
		overflow: visible;
	}
	.info-inline > .badge-line {
		flex: 0 0 auto;
	}
	/* The mono key preview is rendered by the Text atom, so this layout rule must
	   be :global to reach it; ellipsis is handled by Text's `truncate` prop. */
	.info-inline > :global(.mono) {
		flex: 0 1 auto;
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
	.mini {
		gap: var(--sp-1);
	}
	.perm {
		gap: var(--sp-3);
	}
</style>
