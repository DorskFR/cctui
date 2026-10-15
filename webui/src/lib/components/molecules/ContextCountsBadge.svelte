<script lang="ts">
	import type { ContextCounts } from '@bindings/ContextCounts';
	import { Badge } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let { counts }: { counts: ContextCounts } = $props();
	const label = $derived(m.conversation_context_chip(counts));
</script>

<Badge
	tone="muted"
	size="xs"
	icon="book"
	numeric
	style="flex:none"
	title={label}
	data-journey="card-context">{counts.rules + counts.memories}</Badge
>
