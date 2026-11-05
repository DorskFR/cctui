<script lang="ts">
	import { Button, EmptyState } from '@dorsk/tsumikit';
	import { archivedSearchHref } from './missing';
	import { m } from '$lib/paraglide/messages';

	let { id, onclose }: { id: string; onclose: () => void } = $props();
</script>

<div class="missing" data-testid="session-missing">
	<EmptyState icon="archive" title={m.chrome_missing_title()} description={m.chrome_missing_body({ id })}>
		{#snippet action()}
			<div class="actions">
				<Button variant="primary" href={archivedSearchHref()}>{m.chrome_missing_search()}</Button>
				<Button variant="ghost" onclick={onclose}>{m.chrome_missing_back()}</Button>
			</div>
		{/snippet}
	</EmptyState>
</div>

<style>
	.missing {
		display: grid;
		place-items: center;
		height: 100%;
		padding: var(--sp-6) var(--sp-4);
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		justify-content: center;
		gap: var(--sp-2);
	}
</style>
