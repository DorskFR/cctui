<script lang="ts">
	// Asks a machine's daemon to re-run codex `model/list`. The catalog lands
	// asynchronously over the daemon WS, so the pickers' queries are invalidated
	// after a short grace period rather than on the POST's reply.
	import { IconButton } from '@dorsk/tsumikit';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { endpoints } from '$lib/queries';
	import { errMessage } from '$lib/api';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	let { machineId, size = 18 }: { machineId: string; size?: number } = $props();

	// Rendered without a provider (tests, previews): the button still posts,
	// it just cannot invalidate.
	let qc: ReturnType<typeof useQueryClient> | null = null;
	try {
		qc = useQueryClient();
	} catch {
		qc = null;
	}
	let busy = $state(false);

	async function refresh() {
		if (busy || !machineId) return;
		busy = true;
		try {
			await endpoints.refreshCodexModels(machineId);
			toasts.ok(m.codex_models_refresh_sent());
			setTimeout(() => {
				qc?.invalidateQueries({ queryKey: ['codex-models'] });
				busy = false;
			}, 4000);
		} catch (e) {
			toasts.error(errMessage(e));
			busy = false;
		}
	}
</script>

<IconButton icon={busy ? 'loader' : 'retry'} label={m.codex_models_refresh()} disabled={busy} onclick={refresh} {size} />
