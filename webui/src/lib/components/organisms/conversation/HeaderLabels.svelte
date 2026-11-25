<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import LabelBadge from '#lib/components/molecules/LabelBadge.svelte';
	import type { LabelHandlers } from './headerLabels';

	let { session, handlers }: { session: SessionListItem; handlers: LabelHandlers } = $props();
</script>

<LabelBadge
	labels={session.labels}
	editable={!!handlers.onAttachLabel}
	allLabels={handlers.allLabels ?? []}
	onCreate={handlers.onCreateLabel}
	onAttach={(lid) => handlers.onAttachLabel?.(session.id, lid)}
	onDetach={(lid) => handlers.onDetachLabel?.(session.id, lid)}
	onUpdate={handlers.onUpdateLabel}
	onDelete={handlers.onDeleteLabel}
/>
