<script module lang="ts">
	export const received: Record<string, unknown>[] = [];
</script>

<script lang="ts">
	let props: Record<string, unknown> = $props();
	received.push(props);
</script>

<div data-hmm-stub></div>
