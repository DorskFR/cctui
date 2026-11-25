<script lang="ts">
	import { untrack } from 'svelte';
	import SpawnAutoArchive from './SpawnAutoArchive.svelte';

	let { checked = false, onready }: { checked?: boolean; onready: (form: { auto_archive: boolean }) => void } =
		$props();

	let form = $state({ auto_archive: untrack(() => checked) });
	untrack(() => onready(form));
</script>

<SpawnAutoArchive bind:checked={form.auto_archive} />
