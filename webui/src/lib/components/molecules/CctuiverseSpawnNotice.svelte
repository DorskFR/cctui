<script lang="ts">
	import { Callout, Field, Input } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let { label = $bindable(''), placeholder }: { label?: string; placeholder: string } = $props();
</script>

<Callout tone="info" title={m.cctuiverse_spawn_notice()} style="flex-basis:100%">
	<Field label={m.cctuiverse_label_label()} hint={m.cctuiverse_label_hint()}>
		<Input bind:value={label} {placeholder} maxlength={80} size="sm" />
	</Field>
</Callout>
