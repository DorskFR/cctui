<script lang="ts">
	// Generic yes/no confirmation dialog in the cctui house style (scrim +
	// centered panel, same as AccountSwitchModal/ForkModal). Replaces `confirm()`
	// for destructive bulk actions so the prompt is themed, translatable and
	// keyboard-dismissable instead of a browser chrome popup.
	//
	// The confirm handler may be async: while it is in flight both buttons are
	// disabled and the confirm button spins, so a slow batch call can't be
	// double-submitted. Closing is left to the caller's `onclose` (we call it
	// ourselves after a successful confirm).
	import { Button, Heading, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		title,
		body,
		confirmLabel = m.common_yes(),
		cancelLabel = m.common_no(),
		danger = true,
		onconfirm,
		onclose
	}: {
		title: string;
		/** One line of explanation; the caller interpolates any count. */
		body?: string;
		confirmLabel?: string;
		cancelLabel?: string;
		/** Style the confirm button as destructive (the default for this dialog). */
		danger?: boolean;
		/** Run the action. Awaited: rejections keep the dialog open. */
		onconfirm: () => void | Promise<void>;
		onclose: () => void;
	} = $props();

	let busy = $state(false);

	async function confirm() {
		if (busy) return;
		busy = true;
		try {
			await onconfirm();
			onclose();
		} finally {
			busy = false;
		}
	}

	// Escape cancels, Enter confirms — standard dialog affordances. Bound at the
	// window so the dialog responds without needing focus inside it.
	function onkeydown(e: KeyboardEvent) {
		if (busy) return;
		if (e.key === 'Escape') {
			e.preventDefault();
			onclose();
		} else if (e.key === 'Enter') {
			e.preventDefault();
			void confirm();
		}
	}
</script>

<svelte:window {onkeydown} />

<div
	class="confirm-scrim"
	role="button"
	tabindex="-1"
	aria-label={cancelLabel}
	onclick={() => !busy && onclose()}
	onkeydown={() => {}}
></div>
<div class="confirm-modal" role="dialog" aria-modal="true" aria-label={title}>
	<Heading level={3}>{title}</Heading>
	{#if body}
		<Text as="p" tone="muted" size="sm">{body}</Text>
	{/if}
	<div class="confirm-foot">
		<Button size="sm" variant="ghost" disabled={busy} onclick={onclose}>{cancelLabel}</Button>
		<Button
			size="sm"
			variant={danger ? 'danger' : 'default'}
			disabled={busy}
			loading={busy}
			onclick={confirm}
		>
			{confirmLabel}
		</Button>
	</div>
</div>

<style>
	.confirm-scrim {
		position: fixed;
		inset: 0;
		z-index: 70;
		background: rgba(0, 0, 0, 0.45);
		border: none;
	}
	.confirm-modal {
		position: fixed;
		z-index: 71;
		top: 50%;
		left: 50%;
		transform: translate(-50%, -50%);
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		width: min(24rem, calc(100vw - 2rem));
		padding: var(--sp-4);
		background: var(--bg-elevated);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-lg, var(--r-md));
		box-shadow: var(--shadow-lg, 0 8px 24px rgba(0, 0, 0, 0.5));
	}
	.confirm-foot {
		display: flex;
		justify-content: flex-end;
		gap: var(--sp-2);
		margin-top: var(--sp-1);
	}
</style>
