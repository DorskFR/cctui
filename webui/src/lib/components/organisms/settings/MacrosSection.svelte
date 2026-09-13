<script lang="ts">
	// Settings › Macros: a switch that puts the ⚡ Macros menu on the Sessions
	// toolbar, and the list of macros. A macro is a prompt plus the spawn knobs
	// it runs with (harness, machine, working directory, model, effort, pool,
	// permission mode) and whether it asks before running. Persisted in the
	// settings blob; the server clamps shape and size and never runs one.
	import { Button, Input, Select, Switch, Text, Textarea } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import { useAccountPools, useAllMachines } from '$lib/queries';
	import { settings, type MacroSpec } from '$lib/settings.svelte';
	import { m } from '$lib/paraglide/messages';
	import { modes } from '../spawn/options';
	import { claudeModels } from '$lib/harnessModels';
	import { effortsFor, macroProblems, newMacro } from '../macros.logic';

	const machinesQ = useAllMachines(() => true);
	const poolsQ = useAccountPools(() => true);
	const machines = $derived((machinesQ.data ?? []).filter((r) => r.kind !== 'ephemeral'));
	const pools = $derived(poolsQ.data ?? []);
	const enabled = $derived(settings.macrosEnabled);
	const list = $derived(settings.macros);

	// The row being edited (a copy: nothing lands until Save).
	let draft = $state<MacroSpec | null>(null);
	const problems = $derived(draft ? macroProblems(draft) : []);

	const machineName = (id: string | null) => {
		const row = machines.find((r) => r.id === id);
		return row ? (row.display_name ?? row.name) : (id ?? '');
	};
	const poolLabel = (id: string | null) => pools.find((p) => p.id === id)?.name ?? m.spawn_account_auto();

	function startNew() {
		draft = newMacro(crypto.randomUUID());
	}
	function edit(mac: MacroSpec) {
		draft = { ...mac };
	}
	function save() {
		if (!draft || problems.length) return;
		const next = { ...draft, title: draft.title.trim(), working_dir: draft.working_dir?.trim() ?? null };
		const idx = list.findIndex((x) => x.id === next.id);
		settings.setMacros(idx < 0 ? [...list, next] : list.map((x, i) => (i === idx ? next : x)));
		draft = null;
	}
	function remove(id: string) {
		settings.setMacros(list.filter((x) => x.id !== id));
		if (draft?.id === id) draft = null;
	}
	const sel = (e: Event) => (e.currentTarget as HTMLSelectElement).value;
	const inp = (e: Event) => (e.currentTarget as HTMLInputElement | HTMLTextAreaElement).value;
	const orNull = (v: string) => (v.trim() ? v.trim() : null);
</script>

<SettingSection id="macros" icon="⚡" title={m.settings_nav_macros()}>
	{#snippet descriptionSlot()}
		<Text size="sm" tone="faint">{m.settings_macros_intro()}</Text>
	{/snippet}
	<SettingGroup>
		<SettingRow label={m.settings_macros_enabled_label()} help={m.settings_macros_enabled_help()}>
			<Switch
				bind:checked={() => enabled, (v) => settings.setMacrosEnabled(v)}
				label={m.settings_macros_enabled_label()}
			/>
		</SettingRow>
	</SettingGroup>

	<SettingGroup title={m.settings_macros_list_title()}>
		<SettingRow label={m.settings_macros_list_label()} help={m.settings_macros_list_help()} wide selfLabelled>
			{#if list.length === 0}
				<Text size="sm" tone="faint">{m.settings_macros_empty()}</Text>
			{:else}
				<ul class="list">
					{#each list as mac (mac.id)}
						<li class="item" data-setting-row>
							<span class="main">
								<span class="title">{mac.title}</span>
								<span class="meta">
									<Text size="xs" tone="faint"
										>{mac.adapter} · {machineName(mac.machine_id)} · {mac.working_dir ?? ''}</Text
									>
									<Text size="xs" tone="faint"
										>{mac.model ?? m.spawn_effort_default()} · {mac.effort ?? m.spawn_effort_default()} · {poolLabel(mac.pool_id)} · {mac.confirm
											? m.settings_macros_confirm_yes()
											: m.settings_macros_confirm_no()}</Text
									>
								</span>
							</span>
							<span class="acts">
								<Button size="sm" onclick={() => edit(mac)}>{m.common_edit()}</Button>
								<Button size="sm" variant="danger" onclick={() => remove(mac.id)}>{m.common_delete()}</Button>
							</span>
						</li>
					{/each}
				</ul>
			{/if}
			{#if !draft}
				<div class="add">
					<Button variant="primary" onclick={startNew}>{m.settings_macros_add()}</Button>
				</div>
			{/if}
		</SettingRow>
	</SettingGroup>

	{#if draft}
		<SettingGroup title={list.some((x) => x.id === draft?.id) ? m.settings_macros_edit_title() : m.settings_macros_new_title()}>
			<SettingRow label={m.settings_macros_field_title()}>
				<Input style="width:100%" value={draft.title} oninput={(e) => (draft!.title = inp(e))} />
			</SettingRow>
			<SettingRow label={m.settings_macros_field_prompt()} wide>
				<Textarea
					style="width:100%"
					rows={6}
					value={draft.prompt}
					oninput={(e) => (draft!.prompt = inp(e))}
				/>
			</SettingRow>
			<SettingRow label={m.settings_macros_field_adapter()}>
				<Select
					style="width:100%"
					value={draft.adapter}
					onchange={(e) => {
						draft!.adapter = sel(e);
						draft!.effort = null;
						draft!.model = null;
					}}
				>
					<option value="claude-code">Claude Code</option>
					<option value="codex">Codex</option>
				</Select>
			</SettingRow>
			<SettingRow label={m.spawn_machine_label()}>
				<Select style="width:100%" value={draft.machine_id ?? ''} onchange={(e) => (draft!.machine_id = orNull(sel(e)))}>
					<option value="">{m.settings_macros_pick_machine()}</option>
					{#each machines as r (r.id)}
						<option value={r.id}>{r.display_name ?? r.name}</option>
					{/each}
				</Select>
			</SettingRow>
			<SettingRow label={m.spawn_cwd_label()}>
				<Input
					style="width:100%"
					value={draft.working_dir ?? ''}
					placeholder="/home/me/project"
					oninput={(e) => (draft!.working_dir = orNull(inp(e)))}
				/>
			</SettingRow>
			<SettingRow label={m.settings_macros_field_model()} help={m.settings_macros_field_model_help()}>
				{#if draft.adapter === 'claude-code'}
					<Select style="width:100%" value={draft.model ?? ''} onchange={(e) => (draft!.model = orNull(sel(e)))}>
						{#each claudeModels as opt (opt.v)}
							<option value={opt.v}>{opt.label ?? opt.v}</option>
						{/each}
					</Select>
				{:else}
					<Input
						style="width:100%"
						value={draft.model ?? ''}
						placeholder={m.spawn_effort_default()}
						oninput={(e) => (draft!.model = orNull(inp(e)))}
					/>
				{/if}
			</SettingRow>
			<SettingRow label={m.spawn_effort_label()}>
				<Select style="width:100%" value={draft.effort ?? ''} onchange={(e) => (draft!.effort = orNull(sel(e)))}>
					<option value="">{m.spawn_effort_default()}</option>
					{#each effortsFor(draft.adapter) as lv (lv)}
						<option value={lv}>{lv}</option>
					{/each}
				</Select>
			</SettingRow>
			<SettingRow label={m.settings_macros_field_pool()} help={m.settings_macros_field_pool_help()}>
				<Select style="width:100%" value={draft.pool_id ?? ''} onchange={(e) => (draft!.pool_id = orNull(sel(e)))}>
					<option value="">{m.spawn_account_auto()}</option>
					{#each pools as p (p.id)}
						<option value={p.id}>{p.name}</option>
					{/each}
				</Select>
			</SettingRow>
			<SettingRow label={m.settings_macros_field_mode()}>
				<Select
					style="width:100%"
					value={draft.permission_mode ?? ''}
					onchange={(e) => (draft!.permission_mode = orNull(sel(e)))}
				>
					<option value="">{m.spawn_effort_default()}</option>
					{#each modes as md (md.v)}
						<option value={md.v}>{md.label}</option>
					{/each}
				</Select>
			</SettingRow>
			<SettingRow label={m.settings_macros_field_confirm()} help={m.settings_macros_field_confirm_help()}>
				<Switch
					bind:checked={() => draft?.confirm ?? true, (v) => (draft!.confirm = v)}
					label={m.settings_macros_field_confirm()}
				/>
			</SettingRow>
			<SettingRow label="" wide selfLabelled>
				<div class="acts">
					<Button onclick={() => (draft = null)}>{m.common_cancel()}</Button>
					<Button variant="primary" disabled={problems.length > 0} onclick={save}>{m.common_save()}</Button>
					{#if problems.length}
						<Text size="xs" tone="faint">{m.settings_macros_incomplete()}</Text>
					{/if}
				</div>
			</SettingRow>
		</SettingGroup>
	{/if}
</SettingSection>

<style>
	.list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.item {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-3);
		flex-wrap: wrap;
	}
	.main {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
	.title {
		font-weight: 600;
	}
	.meta {
		display: flex;
		flex-direction: column;
	}
	.acts {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
	.add {
		margin-top: var(--sp-3);
	}
</style>
