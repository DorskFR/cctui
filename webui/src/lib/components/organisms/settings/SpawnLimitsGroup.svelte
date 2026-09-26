<script lang="ts">
	import { Button, Input, Text } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import { endpoints } from '$lib/queries';
	import type { SettingSource } from '@bindings/SettingSource';
	import type { SpawnDefaults } from '@bindings/SpawnDefaults';
	import type { SpawnDefaultsInfo } from '@bindings/SpawnDefaultsInfo';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';
	import { parseSpawnDraft, sourceLabel } from './serverSettings.logic';

	type Field = keyof SpawnDefaults;
	const FIELDS: { key: Field; label: () => string }[] = [
		{ key: 'max_children', label: m.settings_spawn_max_children },
		{ key: 'max_depth', label: m.settings_spawn_max_depth },
		{ key: 'max_tree_budget_usd', label: m.settings_spawn_max_budget }
	];

	let info = $state<SpawnDefaultsInfo | null>(null);
	let draft = $state<Record<Field, string>>({ max_children: '', max_depth: '', max_tree_budget_usd: '' });
	let saving = $state(false);

	function apply(next: SpawnDefaultsInfo) {
		info = next;
		for (const { key } of FIELDS) draft[key] = next.settings[key]?.toString() ?? '';
	}

	$effect(() => {
		endpoints
			.spawnDefaults()
			.then(apply)
			.catch(() => {});
	});

	const parsed = $derived(parseSpawnDraft(draft));
	const dirty = $derived(
		!!info && FIELDS.some(({ key }) => draft[key].trim() !== (info?.settings[key]?.toString() ?? ''))
	);

	async function save(next: SpawnDefaults) {
		saving = true;
		try {
			apply(await endpoints.setSpawnDefaults(next));
			toasts.ok(m.settings_spawn_limits_saved());
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			saving = false;
		}
	}

	function effective(key: Field): string {
		if (!info) return '';
		const source: SettingSource = info.sources[key];
		return m.settings_spawn_effective({ value: String(info.effective[key] ?? ''), source: sourceLabel(source) });
	}

	function fallback(key: Field): string {
		return String(info?.env[key] ?? info?.defaults[key] ?? '');
	}
</script>

<div id="spawn-limits">
	<SettingGroup title={m.settings_spawn_limits_label()}>
		<SettingRow label={m.settings_spawn_limits_label()} help={m.settings_spawn_limits_help()} server admin wide selfLabelled>
			<div class="fields">
				{#each FIELDS as f (f.key)}
					<div class="field">
						<Text size="sm">{f.label()}</Text>
						<Input
							bind:value={draft[f.key]}
							inputmode={f.key === 'max_tree_budget_usd' ? 'decimal' : 'numeric'}
							placeholder={fallback(f.key)}
							aria-label={f.label()}
						/>
						<Text size="xs" tone="faint" variant="code">{effective(f.key)}</Text>
						{#if info?.sources[f.key] === 'settings'}
							<Button
								size="sm"
								variant="ghost"
								disabled={saving}
								title={m.settings_upstreams_reset_help()}
								onclick={() => info && save({ ...info.settings, [f.key]: null })}
							>
								{m.settings_reset()}
							</Button>
						{/if}
					</div>
				{/each}
				{#if !parsed}
					<Text size="xs" tone="danger">{m.settings_spawn_limits_invalid()}</Text>
				{/if}
				<div>
					<Button disabled={!dirty || !parsed || saving} onclick={() => parsed && save(parsed)}>
						{m.settings_admin_instance_save()}
					</Button>
				</div>
			</div>
		</SettingRow>
	</SettingGroup>
</div>

<style>
	.fields {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
	}
	.field {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
</style>
