<script lang="ts">
	// Desktop side-pane scrim (extracted from ConversationDrawer): a dim overlay
	// over the rest of the viewport so clicking outside the pane (or Escape)
	// closes it, instead of hunting for the ‹ icon. Hidden on mobile where the
	// drawer is full-width.
	let { onclose }: { onclose: () => void } = $props();
</script>

<div
	class="scrim"
	role="button"
	tabindex="-1"
	aria-label="Close conversation"
	onclick={onclose}
	onkeydown={(e) => e.key === 'Escape' && onclose()}
></div>

<style>
	/* No scrim on mobile — the drawer is full-width, nothing behind to click. */
	.scrim {
		display: none;
	}
	@media (min-width: 960px) {
		.scrim {
			display: block;
			position: fixed;
			inset: 0;
			z-index: var(--z-drawer);
			background: rgba(0, 0, 0, 0.35);
			animation: fade 0.18s var(--ease);
		}
	}
	@keyframes fade {
		from {
			opacity: 0;
		}
	}
</style>
