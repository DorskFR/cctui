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

	const qc = useQueryClient();
	const machines = useAllMachines(() => true);

	let rows = $state<Record<string, MachineAdapterInfo[]>>({});
	let busy = $state<string | null>(null);

	async function load(machine: MachineRow) {
		try {
			rows[machine.id] = await endpoints.machineAdapters(machine.id);
		} catch {
			rows[machine.id] = [];
		}
	}

	$effect(() => {
		for (const machine of machines.data ?? []) if (!(machine.id in rows)) void load(machine);
	});

	async function run(machine: MachineRow, call: () => Promise<MachineAdapterInfo[]>) {
		busy = machine.id;
		try {
			rows[machine.id] = await call();
			await qc.invalidateQueries({ queryKey: qk.machineAdapters(machine.id) });
			toasts.ok(m.settings_machine_adapters_saved({ name: machine.display_name ?? machine.name }));
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			busy = null;
		}
	}

	const label = (a: MachineAdapterInfo) => harnessLabel(a.adapter_id, harnessTable());
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
								<li class="adapter">
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
