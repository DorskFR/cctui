<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { Button } from '@dorsk/tsumikit';
	import { errMessage } from '#lib/api.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { useSessionActions } from '#lib/queries/sessionActions.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { queuedMachineOf } from './view';

	let { session }: { session: SessionListItem } = $props();
	const actions = useSessionActions();
	let cancelling = $state(false);

	async function cancel() {
		cancelling = true;
		try {
			await actions.cancelQueuedSpawn(session.id);
		} catch (e) {
			toasts.error(m.sessions_queued_cancel_failed({ error: errMessage(e) }));
		} finally {
			cancelling = false;
		}
	}
</script>

<span
	class="queued"
	role="presentation"
	data-journey="queued-spawn"
	onpointerdown={(e) => e.stopPropagation()}
	onclick={(e) => e.stopPropagation()}
>
	<span class="waiting">{m.sessions_queued_waiting({ machine: queuedMachineOf(session) ?? '' })}</span>
	<Button size="sm" loading={cancelling} disabled={cancelling} onclick={cancel}>
		{m.sessions_queued_cancel()}
	</Button>
</span>

<style>
	.queued {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		flex: none;
	}
	.waiting {
		font-size: var(--fs-xs);
		color: var(--text-muted);
		white-space: nowrap;
	}
</style>
