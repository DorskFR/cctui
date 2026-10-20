<script lang="ts">
	import { Badge, Button, Switch, Text } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import BrandLogo from '$lib/components/atoms/BrandLogo.svelte';
	import { endpoints, qk, useAllMachines } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import type { MachineAdapterInfo } from '@bindings/MachineAdapterInfo';
	import type { MachineRow } from '@bindings/MachineRow';
	import { harnessLabel } from '$lib/harnesses';
	import { harnessTable } from '$lib/harnesses.svelte';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';
	import AdapterConfigEditor from './AdapterConfigEditor.svelte';
	import { binaryStatus } from './machineAdapters.logic';

	const qc = useQueryClient();
	const machines = useAllMachines(() => true);

	let rows = $state<Record<string, MachineAdapterInfo[]>>({});
	let busy = $state<string | null>(null);
	const requested = new Set<string>();

	async function load(machine: MachineRow) {
		requested.add(machine.id);
		let list: MachineAdapterInfo[];
		try {
			list = await endpoints.machineAdapters(machine.id);
		} catch {
			list = [];
		}
		if (!(machine.id in rows)) rows[machine.id] = list;
	}

	$effect(() => {
		for (const machine of machines.data ?? []) if (!requested.has(machine.id)) void load(machine);
	});

	async function run(
		machine: MachineRow,
		call: () => Promise<MachineAdapterInfo[]>
	): Promise<boolean> {
		busy = machine.id;
		try {
			rows[machine.id] = await call();
			await qc.invalidateQueries({ queryKey: qk.machineAdapters(machine.id) });
			toasts.ok(m.settings_machine_adapters_saved({ name: machine.display_name ?? machine.name }));
			return true;
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
			return false;
		} finally {
			busy = null;
		}
	}

	const label = (a: MachineAdapterInfo) => harnessLabel(a.adapter_id, harnessTable());
	const ABOUT: Record<string, () => string> = {
		qwen: m.settings_machine_adapters_about_qwen,
		goose: m.settings_machine_adapters_about_goose
	};
	const about = (a: MachineAdapterInfo) => ABOUT[a.adapter_id]?.();
</script>

<SettingGroup title={m.settings_machine_adapters_label()}>
	<SettingRow
		label={m.settings_machine_adapters_label()}
		help={m.settings_machine_adapters_help()}
		server
		admin
		wide
		selfLabelled
	>
		{#if (machines.data ?? []).length === 0}
			<Text size="sm" tone="faint">{m.settings_machine_adapters_no_machines()}</Text>
		{:else}
			<ul class="machines">
				{#each machines.data ?? [] as machine (machine.id)}
					{@const list = rows[machine.id] ?? []}
					<li class="machine">
						<Text size="sm" weight="semibold">{machine.display_name ?? machine.name}</Text>
						<ul class="adapters">
							{#each list as a (a.adapter_id)}
								<li class="adapter" title={about(a)}>
									<BrandLogo adapter={a.adapter_id} size={14} />
									<Switch
										checked={a.enabled}
										disabled={busy === machine.id}
										label={m.settings_machine_adapters_toggle_aria({
											harness: label(a),
											name: machine.display_name ?? machine.name
										})}
										labelVisible
										size="sm"
										onclick={() =>
											run(machine, () =>
												endpoints.setMachineAdapter(machine.id, a.adapter_id, { enabled: !a.enabled })
											)}
									/>
									{#if a.pinned}
										<Button
											size="sm"
											variant="ghost"
											disabled={busy === machine.id}
											onclick={() =>
												run(machine, () => endpoints.resetMachineAdapter(machine.id, a.adapter_id))}
										>
											{m.settings_machine_adapters_reset()}
										</Button>
									{:else}
										<Badge size="sm">{m.settings_machine_adapters_default()}</Badge>
									{/if}
									{#if binaryStatus(a) === 'found'}
										<Badge size="sm" tone="ok" title={a.binary?.path}>
											{m.settings_machine_adapters_binary_found()}
										</Badge>
									{:else if binaryStatus(a) === 'missing'}
										<Badge
											size="sm"
											tone="danger"
											title={m.settings_machine_adapters_binary_missing_hint({
												bin: a.binary?.bin ?? '',
												key: a.binary?.config_key ?? ''
											})}
										>
											{m.settings_machine_adapters_binary_missing({ bin: a.binary?.bin ?? '' })}
										</Badge>
									{/if}
									<AdapterConfigEditor
										config={a.config}
										disabled={busy === machine.id}
										label={m.settings_machine_adapters_config_aria({
											harness: label(a),
											name: machine.display_name ?? machine.name
										})}
										onsave={(config) =>
											run(machine, () =>
												endpoints.setMachineAdapter(machine.id, a.adapter_id, {
													enabled: a.enabled,
													config
												})
											)}
									/>
								</li>
							{/each}
						</ul>
					</li>
				{/each}
			</ul>
		{/if}
	</SettingRow>
</SettingGroup>

<style>
	.machines,
	.adapters {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.machine {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.adapters {
		padding-left: var(--sp-3);
	}
	.adapter {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
</style>
