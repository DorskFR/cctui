<script lang="ts">
	import { Badge, Input, SectionHeader, Text } from '@dorsk/tsumikit';
	import { BUILTIN_SCRUB_DETECTORS, type ScrubFamily } from '$lib/scrubDetectors.generated';
	import { normalizeForFilter } from '$lib/components/organisms/settings/settings.logic';
	import { m } from '$lib/paraglide/messages';

	const FAMILY_ORDER: ScrubFamily[] = ['ai', 'forge', 'cloud', 'registry', 'saas', 'generic'];

	const familyLabel: Record<ScrubFamily, () => string> = {
		ai: m.settings_redaction_family_ai,
		forge: m.settings_redaction_family_forge,
		cloud: m.settings_redaction_family_cloud,
		registry: m.settings_redaction_family_registry,
		saas: m.settings_redaction_family_saas,
		generic: m.settings_redaction_family_generic
	};

	let filter = $state('');

	const groups = $derived.by(() => {
		const needle = normalizeForFilter(filter.trim());
		const matching = needle
			? BUILTIN_SCRUB_DETECTORS.filter((d) => normalizeForFilter(d.category).includes(needle))
			: BUILTIN_SCRUB_DETECTORS;
		return FAMILY_ORDER.map((family) => ({
			family,
			items: matching.filter((d) => d.family === family)
		})).filter((g) => g.items.length > 0);
	});
</script>

<details class="detectors" data-setting-row>
	<summary>
		<Text size="sm" tone="muted">
			{m.settings_redaction_builtins_count({ count: BUILTIN_SCRUB_DETECTORS.length })}
		</Text>
	</summary>
	<div class="body">
		<Input
			type="search"
			size="sm"
			style="width:100%"
			bind:value={filter}
			aria-label={m.settings_redaction_detector_filter_label()}
			placeholder={m.settings_redaction_detector_filter_label()}
		/>
		{#each groups as group (group.family)}
			<SectionHeader title={familyLabel[group.family]()} size="sm" />
			<div class="chips">
				{#each group.items as d (d.category)}
					<Badge mono size="sm" border>{d.category}</Badge>
				{/each}
			</div>
		{:else}
			<Text size="sm" tone="muted">{m.settings_redaction_detector_filter_empty()}</Text>
		{/each}
	</div>
</details>

<style>
	.detectors {
		border-top: 1px solid var(--border);
	}
	.detectors summary {
		padding: var(--sp-2) var(--sp-4);
		cursor: pointer;
	}
	.body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: 0 var(--sp-4) var(--sp-3);
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1);
	}
</style>
