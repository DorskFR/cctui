<script lang="ts">
	// Settings › Context: the memory notes and prompt templates a session can be
	// launched with. Skills are not here — a skill bundle is a plugin, so it
	// lives on the Plugins page.
	import { Button, Input, Select, Switch, Text, Textarea } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import type { ContextItem } from '@bindings/ContextItem';
	import type { ContextItemSpec } from '@bindings/ContextItemSpec';
	import { useAllMachines, useContextActions, useContextItems } from '$lib/queries';
	import { m } from '$lib/paraglide/messages';
	import {
		CONTEXT_SCOPES,
		contextProblems,
		newContextItem,
		scopeSummary,
		slugify,
		toSpec,
		type ContextKind
	} from './context.logic';

	const itemsQ = useContextItems();
	const machinesQ = useAllMachines(() => true);
	const actions = useContextActions();

	const machines = $derived((machinesQ.data ?? []).filter((r) => r.kind !== 'ephemeral'));
	const items = $derived(itemsQ.data ?? []);
	const memories = $derived(items.filter((i) => i.kind === 'memory'));
	const prompts = $derived(items.filter((i) => i.kind === 'prompt'));

	// The row being edited: a copy, so nothing lands until Save.
	let draft = $state<ContextItemSpec | null>(null);
	let editing = $state<string | null>(null);
	let busy = $state(false);
	let error = $state<string | null>(null);
	const problems = $derived(draft ? contextProblems(draft) : []);

	function startNew(kind: ContextKind) {
		editing = null;
		error = null;
		draft = newContextItem(kind);
	}
	function edit(item: ContextItem) {
		editing = item.id;
		error = null;
		draft = toSpec(item);
	}
	function cancel() {
		draft = null;
		editing = null;
		error = null;
	}
	async function save() {
		if (!draft || problems.length || busy) return;
		busy = true;
		error = null;
		try {
			if (editing) await actions.update(editing, draft);
			else await actions.create(draft);
			cancel();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}
	async function remove(id: string) {
		busy = true;
		try {
			await actions.remove(id);
			if (editing === id) cancel();
		} finally {
			busy = false;
		}
	}

	const inp = (e: Event) => (e.currentTarget as HTMLInputElement | HTMLTextAreaElement).value;
	const sel = (e: Event) => (e.currentTarget as HTMLSelectElement).value;
	const orNull = (v: string) => (v.trim() ? v.trim() : null);

	function onTitle(v: string) {
		if (!draft) return;
		const wasSuggested = draft.name === slugify(draft.title ?? '');
		draft.title = v;
		if (!editing && (!draft.name || wasSuggested)) draft.name = slugify(v);
	}
</script>

<SettingSection id="context" icon="❖" title={m.settings_nav_context()}>
	{#snippet descriptionSlot()}
		<Text size="sm" tone="faint">{m.settings_context_intro()}</Text>
	{/snippet}

	{#each [{ kind: 'memory' as const, rows: memories, title: m.settings_context_memory_title(), help: m.settings_context_memory_help(), add: m.settings_context_add_memory() }, { kind: 'prompt' as const, rows: prompts, title: m.settings_context_prompts_title(), help: m.settings_context_prompts_help(), add: m.settings_context_add_prompt() }] as bucket (bucket.kind)}
		<SettingGroup title={bucket.title}>
			<SettingRow label={bucket.title} help={bucket.help} wide selfLabelled>
				{#if bucket.rows.length === 0}
					<Text size="sm" tone="faint">{m.settings_context_empty()}</Text>
				{:else}
					<ul class="list">
						{#each bucket.rows as item (item.id)}
							<li class="item" data-setting-row>
								<span class="main">
									<span class="title">{item.title}</span>
									<span class="meta">
										<Text size="xs" tone="faint"
											>{item.name} · {scopeSummary(item)} · v{item.version}{item.enabled
												? ''
												: ` · ${m.settings_context_disabled()}`}</Text
										>
									</span>
								</span>
								<span class="acts">
									<Button size="sm" disabled={busy} onclick={() => edit(item)}>{m.common_edit()}</Button>
									<Button size="sm" variant="danger" disabled={busy} onclick={() => remove(item.id)}
										>{m.common_delete()}</Button
									>
								</span>
							</li>
						{/each}
					</ul>
				{/if}
				{#if !draft}
					<div class="add">
						<Button variant="primary" onclick={() => startNew(bucket.kind)}>{bucket.add}</Button>
					</div>
				{/if}
			</SettingRow>
		</SettingGroup>
	{/each}

	{#if draft}
		<SettingGroup
			title={editing ? m.settings_context_edit_title() : m.settings_context_new_title()}
		>
			<SettingRow label={m.settings_context_field_title()}>
				<Input style="width:100%" value={draft.title} oninput={(e) => onTitle(inp(e))} />
			</SettingRow>
			<SettingRow label={m.settings_context_field_name()} help={m.settings_context_field_name_help()}>
				<Input style="width:100%" value={draft.name} oninput={(e) => (draft!.name = inp(e))} />
			</SettingRow>
			<SettingRow label={m.settings_context_field_body()} wide>
				<Textarea
					style="width:100%"
					rows={8}
					value={draft.body}
					oninput={(e) => (draft!.body = inp(e))}
				/>
			</SettingRow>
			{#if draft.kind === 'prompt'}
				<SettingRow label={m.settings_context_vars_label()} help={m.settings_context_vars_help()} wide>
					<Text size="xs" tone="faint">{'{{cwd}} · {{name}} · {{topic}}'}</Text>
				</SettingRow>
			{/if}
			<SettingRow label={m.settings_context_field_scope()} help={m.settings_context_field_scope_help()}>
				<Select
					style="width:100%"
					value={draft.scope}
					onchange={(e) => {
						draft!.scope = sel(e);
						draft!.scope_ref = null;
					}}
				>
					{#each CONTEXT_SCOPES as s (s)}
						<option value={s}>{s}</option>
					{/each}
				</Select>
			</SettingRow>
			{#if draft.scope === 'machine'}
				<SettingRow label={m.spawn_machine_label()}>
					<Select
						style="width:100%"
						value={draft.scope_ref ?? ''}
						onchange={(e) => (draft!.scope_ref = orNull(sel(e)))}
					>
						<option value="">{m.settings_macros_pick_machine()}</option>
						{#each machines as r (r.id)}
							<option value={r.id}>{r.display_name ?? r.name}</option>
						{/each}
					</Select>
				</SettingRow>
			{:else if draft.scope !== 'user'}
				<SettingRow
					label={draft.scope === 'path' ? m.spawn_cwd_label() : m.settings_context_field_label()}
				>
					<Input
						style="width:100%"
						value={draft.scope_ref ?? ''}
						placeholder={draft.scope === 'path' ? '/home/me/project' : ''}
						oninput={(e) => (draft!.scope_ref = orNull(inp(e)))}
					/>
				</SettingRow>
			{/if}
			<SettingRow label={m.settings_context_field_enabled()} help={m.settings_context_field_enabled_help()}>
				<Switch
					bind:checked={() => draft!.enabled ?? true, (v) => (draft!.enabled = v)}
					label={m.settings_context_field_enabled()}
				/>
			</SettingRow>
			{#if problems.length || error}
				<SettingRow label={m.settings_context_problems()} wide selfLabelled>
					<Text size="sm" tone="danger">{error ?? m.settings_context_invalid()}</Text>
				</SettingRow>
			{/if}
			<SettingRow label={m.common_save()} wide selfLabelled>
				<div class="acts">
					<Button variant="primary" disabled={busy || problems.length > 0} onclick={save}
						>{m.common_save()}</Button
					>
					<Button disabled={busy} onclick={cancel}>{m.common_cancel()}</Button>
				</div>
			</SettingRow>
		</SettingGroup>
	{/if}
</SettingSection>

<style>
	.list {
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
		margin: 0;
		padding: 0;
		list-style: none;
		width: 100%;
	}
	.item {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 0.75rem;
		padding: 0.4rem 0.6rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
	}
	.main {
		display: flex;
		flex-direction: column;
		gap: 0.15rem;
		min-width: 0;
	}
	.title {
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.meta {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
	}
	.acts {
		display: flex;
		gap: 0.4rem;
		flex: 0 0 auto;
	}
	.add {
		margin-top: 0.6rem;
	}
</style>
