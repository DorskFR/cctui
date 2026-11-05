<script lang="ts">
	// Instance › General: the deployment name shown in the header, what this
	// browser pulls from the API, server-wide sign-in and the machine the
	// self-update agent runs on. Admin values live in `instance_settings` on the
	// server, not in the per-user blob; the name is read back through /version
	// so the header and tab title pick it up on the next refetch. Every field
	// saves when committed (blur, Enter or a pick); there is no Save button.
	import { Input, Select } from '@dorsk/tsumikit';
	import { useQueryClient } from '@tanstack/svelte-query';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import MachinePicker from '$lib/components/molecules/MachinePicker.svelte';
	import NetStatsChip from '$lib/components/molecules/NetStatsChip.svelte';
	import PasskeySignInGroup from './PasskeySignInGroup.svelte';
	import { useVersion, useAllMachines, endpoints, qk } from '$lib/queries';
	import type { SelfUpdateTargetInfo } from '@bindings/SelfUpdateTargetInfo';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';
	import { selfUpdateCommit } from './instance.logic';

	const version = useVersion();
	const qc = useQueryClient();

	let instanceDraft = $state('');
	$effect(() => {
		instanceDraft = version.data?.instance_name ?? '';
	});

	async function saveInstanceName() {
		const next = instanceDraft.trim();
		if (next === (version.data?.instance_name ?? '')) return;
		try {
			const res = await endpoints.updateInstance(next || null);
			instanceDraft = res.name ?? '';
			await qc.invalidateQueries({ queryKey: qk.version });
			toasts.ok(res.name ? m.settings_admin_instance_saved() : m.settings_admin_instance_cleared());
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		}
	}

	// Self-update target: which enrolled machine + directory the "Update"
	// button hands the deployment to. The server never learns how cctui is
	// deployed there — the agent reads that machine's own notes.
	const allMachines = useAllMachines(() => true);
	let suTarget = $state<SelfUpdateTargetInfo | null>(null);
	let suMachine = $state('');
	let suDir = $state('');
	let suAdapter = $state('claude-code');
	let suSaving = $state(false);

	function applyTarget(info: SelfUpdateTargetInfo) {
		suTarget = info;
		suMachine = info.target?.machine_id ?? '';
		suDir = info.target?.working_dir ?? '';
		suAdapter = info.target?.adapter_id ?? 'claude-code';
	}

	$effect(() => {
		endpoints
			.selfUpdateTarget()
			.then(applyTarget)
			.catch(() => {});
	});

	async function commitSelfUpdate() {
		const action = selfUpdateCommit(suTarget, { machine: suMachine, dir: suDir, adapter: suAdapter });
		if (action.kind === 'none' || suSaving) return;
		suSaving = true;
		try {
			const info = await endpoints.setSelfUpdateTarget(action.kind === 'clear' ? null : action.target);
			applyTarget(info);
			qc.invalidateQueries({ queryKey: qk.version });
			toasts.ok(action.kind === 'clear' ? m.settings_self_update_cleared() : m.settings_self_update_saved());
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			suSaving = false;
		}
	}
</script>

<SettingSection id="general" icon="⚙" title={m.set_general_title()}>
	<SettingGroup>
		<SettingRow label={m.settings_admin_instance_name_label()} help={m.settings_admin_instance_name_help()}>
			<Input
				bind:value={instanceDraft}
				maxlength={48}
				grow
				placeholder={m.settings_admin_instance_name_placeholder()}
				aria-label={m.settings_admin_instance_name_label()}
				onchange={saveInstanceName}
				onkeydown={(e: KeyboardEvent) => {
					if (e.key === 'Enter') void saveInstanceName();
				}}
			/>
		</SettingRow>
		<SettingRow label={m.net_stats_title()} help={m.settings_net_stats_help()} selfLabelled>
			<NetStatsChip />
		</SettingRow>
	</SettingGroup>

	<PasskeySignInGroup />

	<div id="self-update">
		<SettingGroup>
			<SettingRow
				label={m.settings_self_update_label()}
				help={suTarget?.source === 'env'
					? `${m.settings_self_update_help()} ${m.settings_self_update_from_env()}`
					: `${m.settings_self_update_help()} ${m.set_self_update_clear_hint()}`}
				wide
				selfLabelled
			>
				<div class="su">
					{#if allMachines.data}
						<MachinePicker
							bind:value={suMachine}
							machines={allMachines.data}
							label={m.settings_self_update_machine()}
							onchange={commitSelfUpdate}
						/>
					{/if}
					<Input
						bind:value={suDir}
						grow
						placeholder={m.settings_self_update_dir_placeholder()}
						aria-label={m.settings_self_update_dir()}
						onchange={commitSelfUpdate}
					/>
					<Select
						value={suAdapter}
						aria-label={m.settings_self_update_adapter()}
						onchange={(e) => {
							suAdapter = (e.currentTarget as HTMLSelectElement).value;
							void commitSelfUpdate();
						}}
					>
						<option value="claude-code">claude-code</option>
						<option value="codex">codex</option>
					</Select>
				</div>
			</SettingRow>
		</SettingGroup>
	</div>
</SettingSection>

<style>
	.su {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
	}
</style>
