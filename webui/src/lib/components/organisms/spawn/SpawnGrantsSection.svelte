<script lang="ts">
	import { Checkbox } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import SpawnAddons from './SpawnAddons.svelte';
	import type { SpawnForm } from './spawnForm.svelte';

	let { sf }: { sf: SpawnForm } = $props();
</script>

<SpawnAddons
	bind:labelIds={sf.form.labels}
	bind:envRows={sf.envRows}
	att={sf.att}
	allLabels={sf.allLabels}
	envInvalid={sf.badEnvKeys.length > 0}
	attachments={sf.target === 'machine'}
	labelActions={sf.actions}
/>
{#if sf.followupParent}
	<Checkbox bind:checked={sf.archiveSource} label={m.followup_archive_source()} />
{/if}
