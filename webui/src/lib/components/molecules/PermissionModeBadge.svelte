<script lang="ts">
	import { Badge } from '@dorsk/tsumikit';
	import { normalizePermissionMode, permissionTone } from '$lib/permissionMode';

	let { mode }: { mode?: string | null } = $props();

	const posture = $derived(normalizePermissionMode(mode));
</script>

{#if posture}
	<span class="pmode" data-testid="permission-mode">
		<Badge tone={permissionTone(mode)} mono>{posture}</Badge>
	</span>
{/if}

<style>
	.pmode {
		display: contents;
	}
</style>
