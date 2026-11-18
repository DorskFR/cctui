<script lang="ts">
	import { Button, SegmentedControl, Select, Switch, Text } from '@dorsk/tsumikit';
	import type { SegmentOption } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import MonitoringGroup from './MonitoringGroup.svelte';
	import {
		settings,
		type NavPosition,
		type SpawnDockSettings,
		type SpawnDockSide
	} from '#lib/settings.svelte.ts';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	const sl = $derived(settings.state.sessionList);

	type Panel = {
		key: string;
		label: string;
		help: string;
		dock: SpawnDockSettings;
		set: (patch: Partial<SpawnDockSettings>) => void;
	};
	const panels = $derived<Panel[]>([
		{
			key: 'launch',
			label: m.settings_spawn_title(),
			help: m.settings_spawn_dock_help(),
			dock: settings.spawnDock,
			set: (p) => settings.setSpawnDock(p)
		},
		{
			key: 'stats',
			label: m.settings_stats_title(),
			help: m.settings_stats_dock_help(),
			dock: settings.statsDock,
			set: (p) => settings.setStatsDock(p)
		},
		{
			key: 'conversation',
			label: m.settings_group_conversation(),
			help: m.settings_conversation_dock_help(),
			dock: settings.conversationDock,
			set: (p) => settings.setConversationDock(p)
		}
	]);

	const sideOptions: SegmentOption[] = [
		{ value: 'left', label: m.settings_spawn_dock_side_left() },
		{ value: 'right', label: m.settings_spawn_dock_side_right() }
	];
	const sideOff = sideOptions.map((o) => ({ ...o, disabled: true }));
	const densityOptions: SegmentOption[] = [
		{ value: 'normal', label: m.settings_row_density_normal() },
		{ value: 'compact', label: m.settings_row_density_compact() }
	];
</script>

<SettingSection id="layout" icon="▦" title={m.settings_sec_layout()}>
	<SettingGroup>
		<SettingRow label={m.settings_nav_position_label()} help={m.settings_nav_position_help()}>
			<Select
				value={settings.nav}
				style="width:100%"
				onchange={(e) => settings.setNav((e.currentTarget as HTMLSelectElement).value as NavPosition)}
			>
				<option value="top">{m.settings_nav_position_top()}</option>
				<option value="bottom">{m.settings_nav_position_bottom()}</option>
			</Select>
		</SettingRow>
		<SettingRow label={m.settings_list_width_label()} help={m.settings_list_width_help()}>
			<Select
				value={sl.width}
				style="width:100%"
				onchange={(e) =>
					settings.setSessionList({
						width: (e.currentTarget as HTMLSelectElement).value as typeof sl.width
					})}
			>
				<option value="default">{m.settings_list_width_default()}</option>
				<option value="wide">{m.settings_list_width_wide()}</option>
				<option value="ultra">{m.settings_list_width_ultra()}</option>
				<option value="full">{m.settings_list_width_full()}</option>
			</Select>
		</SettingRow>
		<SettingRow label={m.settings_account_names_label()} help={m.settings_account_names_help()}>
			<Switch
				bind:checked={() => sl.accountNames, (v) => settings.setSessionList({ accountNames: v })}
				label={m.settings_account_names_label()}
			/>
		</SettingRow>
		<SettingRow label={m.settings_row_density_label()} help={m.settings_row_density_help()}>
			<SegmentedControl
				options={densityOptions}
				bind:value={() => sl.density, (v) => settings.setSessionList({ density: v as typeof sl.density })}
			/>
		</SettingRow>
	</SettingGroup>

	<SettingGroup title={m.settings_panels_title()}>
		<SettingRow
			label={m.settings_panels_label()}
			help={m.settings_panels_help()}
			wide
			selfLabelled
			journey="setting"
			journeyKey="conversation-dock"
		>
			<table class="panels" data-testid="settings-panels">
				<thead>
					<tr>
						<th><Text size="xs" tone="faint">{m.settings_panels_col_panel()}</Text></th>
						<th><Text size="xs" tone="faint">{m.settings_panels_col_visible()}</Text></th>
						<th><Text size="xs" tone="faint">{m.settings_spawn_dock_side_label()}</Text></th>
						<th><Text size="xs" tone="faint">{m.settings_panels_col_width()}</Text></th>
					</tr>
				</thead>
				<tbody>
					{#each panels as p (p.key)}
						<tr data-panel={p.key}>
							<td title={p.help}><Text size="sm">{p.label}</Text></td>
							<td>
								<Switch
									bind:checked={() => p.dock.enabled, (v) => p.set({ enabled: v })}
									label={m.settings_panels_visible_named({ panel: p.label })}
								/>
							</td>
							<td>
								<SegmentedControl
									options={p.dock.enabled ? sideOptions : sideOff}
									label={m.settings_panels_side_named({ panel: p.label })}
									control
									bind:value={() => p.dock.side, (v) => p.set({ side: v as SpawnDockSide })}
								/>
							</td>
							<td>
								<span class="width">
									<Text size="sm" variant="code" tone="faint">
										{p.dock.width ? `${p.dock.width}px` : m.settings_panels_width_default()}
									</Text>
									{#if p.dock.width}
										<Button
											size="sm"
											variant="ghost"
											aria-label={m.settings_panels_width_reset_named({ panel: p.label })}
											onclick={() => p.set({ width: undefined })}
										>
											{m.settings_reset()}
										</Button>
									{/if}
								</span>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</SettingRow>
	</SettingGroup>

	{#if hasFeature('machine_spawn')}
		<MonitoringGroup />
	{/if}
</SettingSection>

<style>
	.panels {
		width: 100%;
		border-collapse: collapse;
	}
	.panels th,
	.panels td {
		padding: var(--sp-1) var(--sp-2);
		text-align: left;
		vertical-align: middle;
	}
	.panels tbody tr {
		border-top: 1px solid var(--border);
	}
	.width {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
