<script lang="ts">
	import { Button, type Schema } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { TIME_SCOPES, dirLabel, isScoped, toggleScope, type Scope, type TimeScope } from '$lib/searchScope';

	interface Props {
		rawQuery: string;
		schema: Schema;
		machine?: string | null;
		dir?: string | null;
	}

	let { rawQuery = $bindable(), schema, machine = null, dir = null }: Props = $props();

	const TIME_LABEL: Record<TimeScope, () => string> = {
		'24h': () => m.search_scope_24h(),
		'7d': () => m.search_scope_7d(),
		'30d': () => m.search_scope_30d()
	};

	const chips = $derived.by(() => {
		const out: { scope: Scope; label: string; title?: string }[] = [];
		if (machine) out.push({ scope: { kind: 'machine', value: machine }, label: m.search_scope_machine({ name: machine }) });
		if (dir) out.push({ scope: { kind: 'dir', value: dir }, label: m.search_scope_dir({ name: dirLabel(dir) }), title: dir });
		for (const t of TIME_SCOPES) out.push({ scope: { kind: 'time', value: t }, label: TIME_LABEL[t]() });
		return out;
	});
</script>

<div class="scope-chips" role="group" aria-label={m.search_scope_group()}>
	{#each chips as chip (`${chip.scope.kind}:${chip.scope.value}`)}
		<Button
			chip
			pill
			size="sm"
			pressed={isScoped(rawQuery, schema, chip.scope)}
			title={chip.title}
			data-scope={chip.scope.kind}
			onclick={() => (rawQuery = toggleScope(rawQuery, schema, chip.scope))}
		>
			{chip.label}
		</Button>
	{/each}
</div>

<style>
	.scope-chips {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1);
	}
</style>
