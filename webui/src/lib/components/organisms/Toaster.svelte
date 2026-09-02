<script lang="ts">
	import { toasts } from '$lib/toast.svelte';
	import { Button, Card } from '@dorsk/tsumikit';

	let wrapEl: HTMLDivElement | undefined = $state();

	// Render the toast stack in the top layer so it paints above any open
	// tsumikit Modal (a `showModal()` <dialog>). z-index alone can't win against
	// the top layer, so we drive a manual popover — same pattern as SpawnModal's
	// label menu. Guarded for browsers without the Popover API.
	$effect(() => {
		const el = wrapEl;
		if (!el) return;
		try {
			if (toasts.items.length) el.showPopover();
			else el.hidePopover();
		} catch {
			// Popover API unsupported: toast falls back to its z-index stacking.
		}
	});
</script>

<div class="toast-wrap" bind:this={wrapEl} popover="manual">
	{#each toasts.items as t (t.id)}
		{#if t.action}
			<!-- A toast with an inline action (e.g. Undo) is a plain card: the text
			     still dismisses on click, the button runs the action. Nested buttons
			     are invalid HTML, so this variant can't be `as="button"`. -->
			<Card
				as="div"
				class="toast toast-with-action {t.kind === 'err' ? 'toast-err' : ''} {t.kind === 'ok' ? 'toast-ok' : ''}"
			>
				<button type="button" class="toast-text" onclick={() => toasts.dismiss(t.id)}>{t.text}</button>
				<Button size="sm" variant="ghost" class="toast-action" onclick={() => toasts.act(t.id)}>
					{t.action.label}
				</Button>
			</Card>
		{:else}
			<Card
				as="button"
				class="toast {t.kind === 'err' ? 'toast-err' : ''} {t.kind === 'ok' ? 'toast-ok' : ''}"
				onclick={() => toasts.dismiss(t.id)}
			>
				{t.text}
			</Card>
		{/if}
	{/each}
</div>
