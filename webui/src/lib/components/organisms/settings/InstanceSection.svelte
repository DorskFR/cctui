<script lang="ts">
	// Settings › Instance › General: the instance name shown in the header,
	// the machine the self-update agent runs on, and server-wide sign-in.
	// Values live in `instance_settings`, not in the per-user blob; the name is
	// read back through /version so the header and tab title pick it up on the
	// next refetch.
	import { Button, Input, Select } from '@dorsk/tsumikit';
	import { useQueryClient } from '@tanstack/svelte-query';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import MachinePicker from '#lib/components/molecules/MachinePicker.svelte';
	import PasskeySignInGroup from './PasskeySignInGroup.svelte';
	import { useVersion, useAllMachines, endpoints, qk } from '#lib/queries/index.ts';
	import type { SelfUpdateTargetInfo } from '@bindings/SelfUpdateTargetInfo';
	import { SAVE_DEBOUNCE_MS, serverSave } from './serverSave.svelte.ts';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	let { isAdmin = false }: { isAdmin?: boolean } = $props();
	const selfUpdate = $derived(hasFeature('self_update'));

	const version = useVersion();
	const qc = useQueryClient();

	let instanceDraft = $state('');
	let instanceEditing = $state(false);
	$effect(() => {
		const saved = version.data?.instance_name ?? '';
		if (!instanceEditing) instanceDraft = saved;
	});
	const instanceDirty = $derived(instanceDraft.trim() !== (version.data?.instance_name ?? ''));

	let instanceSending: string | null = null;
	async function saveInstanceName() {
		const name = instanceDraft.trim();
		if (!instanceDirty || name === instanceSending) return;
		instanceSending = name;
		try {
			await serverSave.run(async () => {
				await endpoints.updateInstance(name || null);
				await qc.invalidateQueries({ queryKey: qk.version });
			});
		} finally {
			instanceSending = null;
		}
	}
	$effect(() => {
		if (!instanceDirty) return;
		const t = setTimeout(saveInstanceName, SAVE_DEBOUNCE_MS);
		return () => clearTimeout(t);
	});

	// Self-update target: which enrolled machine + directory the "Update"
	// button hands the deployment to. The server never learns how cctui is
	// deployed there — the agent reads that machine's own notes.
	const allMachines = useAllMachines(() => isAdmin);
	let suTarget = $state<SelfUpdateTargetInfo | null>(null);
	let suMachine = $state('');
	let suDir = $state('');
	let suAdapter = $state('claude-code');
	function applyTarget(info: SelfUpdateTargetInfo) {
		suTarget = info;
		suMachine = info.target?.machine_id ?? '';
		suDir = info.target?.working_dir ?? '';
		suAdapter = info.target?.adapter_id ?? 'claude-code';
	}
	$effect(() => {
		if (!isAdmin || !selfUpdate) return;
		endpoints
			.selfUpdateTarget()
			.then(applyTarget)
			.catch(() => {});
	});
	const suDirty = $derived(
		suTarget !== null &&
			(suMachine !== (suTarget.target?.machine_id ?? '') ||
				suDir.trim() !== (suTarget.target?.working_dir ?? '') ||
				suAdapter !== (suTarget.target?.adapter_id ?? 'claude-code'))
	);
	const suValid = $derived(suMachine !== '' && suDir.trim() !== '');

	async function saveSelfUpdateTarget(clear = false) {
		const info = await serverSave.run(() =>
			endpoints.setSelfUpdateTarget(
				clear ? null : { machine_id: suMachine, working_dir: suDir.trim(), adapter_id: suAdapter }
			)
		);
		if (!info) return;
		suTarget = info;
		if (clear) applyTarget(info);
		qc.invalidateQueries({ queryKey: qk.version });
	}
	$effect(() => {
		if (!suDirty || !suValid) return;
		const t = setTimeout(() => saveSelfUpdateTarget(), SAVE_DEBOUNCE_MS);
		return () => clearTimeout(t);
	});
</script>

<SettingSection id="instance-general" icon="⚙" title={m.settings_sec_general()}>
	{#if isAdmin}
		<SettingGroup>
			<SettingRow
				label={m.settings_admin_instance_name_label()}
				help={m.settings_admin_instance_name_help()}
			>
				<Input
					bind:value={instanceDraft}
					maxlength={48}
					grow
					placeholder={m.settings_admin_instance_name_placeholder()}
					aria-label={m.settings_admin_instance_name_label()}
					onfocus={() => (instanceEditing = true)}
					onblur={() => {
						void saveInstanceName().finally(() => (instanceEditing = false));
					}}
					onkeydown={(e: KeyboardEvent) => {
						if (e.key === 'Enter') void saveInstanceName();
					}}
				/>
			</SettingRow>
		</SettingGroup>
		{#if selfUpdate}
			<SettingGroup>
				<SettingRow
					label={m.settings_self_update_label()}
					help={suTarget?.source === 'env'
						? `${m.settings_self_update_help()} ${m.settings_self_update_from_env()}`
						: m.settings_self_update_help()}
					wide
					selfLabelled
				>
					<div class="su">
						{#if allMachines.data}
							<MachinePicker
								bind:value={suMachine}
								machines={allMachines.data}
								label={m.settings_self_update_machine()}
								variant="field"
							/>
						{/if}
						<Input
							bind:value={suDir}
							grow
							placeholder={m.settings_self_update_dir_placeholder()}
							aria-label={m.settings_self_update_dir()}
						/>
						<Select
							value={suAdapter}
							aria-label={m.settings_self_update_adapter()}
							onchange={(e) => (suAdapter = (e.currentTarget as HTMLSelectElement).value)}
						>
							<option value="claude-code">claude-code</option>
							<option value="codex">codex</option>
						</Select>
						{#if suTarget?.source === 'settings'}
							<div class="su-actions">
								<Button
									size="sm"
									disabled={serverSave.status === 'pending'}
									onclick={() => saveSelfUpdateTarget(true)}
								>
									{m.settings_self_update_clear()}
								</Button>
							</div>
						{/if}
					</div>
				</SettingRow>
			</SettingGroup>
		{/if}
		{#if hasFeature('passkeys')}
			<PasskeySignInGroup />
		{/if}
	{/if}
</SettingSection>

<style>
	.su {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
	}
	.su-actions {
		display: flex;
		gap: var(--sp-2);
	}
</style>
