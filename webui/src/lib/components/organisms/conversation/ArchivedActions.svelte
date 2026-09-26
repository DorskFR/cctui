<script lang="ts">
	// Composer stand-in for an archived session: read-only hint plus the
	// New from same script · Fork · Resume actions.
	import { Button, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		onNewFromScript,
		onFork,
		onResume
	}: {
		onNewFromScript: () => void;
		onFork: () => void;
		onResume: () => void;
	} = $props();
</script>

<div class="archived-actions">
	<div class="hint"><Text tone="muted" size="sm">{m.composer_archived_readonly()}</Text></div>
	<span class="archived-actions-btns">
		<Button onclick={onNewFromScript}>{m.composer_new_from_script()}</Button>
		<Button onclick={onFork}>{m.composer_fork()}</Button>
		<Button variant="primary" onclick={onResume}>{m.composer_resume()}</Button>
	</span>
</div>

<style>
	.archived-actions {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
	}
	/* Right-aligned action cluster. */
	.archived-actions-btns {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin-left: auto;
	}
	.hint {
		text-align: center;
		width: 100%;
	}
</style>
