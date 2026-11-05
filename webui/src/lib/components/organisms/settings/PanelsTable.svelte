<script lang="ts">
	// The launch, stats and conversation docks of the Sessions screen as one
	// table: shown or not, which edge, and the width the grip left them at.
	import { Button, SegmentedControl, Switch, Text } from '@dorsk/tsumikit';
	import type { SegmentOption } from '@dorsk/tsumikit';
	import { settings, type SpawnDockSide } from '$lib/settings.svelte';
	import { m } from '$lib/paraglide/messages';

	type Dock = {
		key: 'launch' | 'stats' | 'conversation';
		label: string;
		state: { enabled: boolean; side: SpawnDockSide; width?: number };
		set: (patch: { enabled?: boolean; side?: SpawnDockSide; width?: number }) => void;
		journey?: string;
	};

	const docks = $derived<Dock[]>([
		{
			key: 'launch',
			label: m.settings_spawn_dock_label(),
			state: settings.spawnDock,
			set: (p) => settings.setSpawnDock(p)
		},
		{
			key: 'stats',
			label: m.settings_stats_dock_label(),
			state: settings.statsDock,
			set: (p) => settings.setStatsDock(p)
		},
		{
			key: 'conversation',
			label: m.settings_conversation_dock_label(),
			state: settings.conversationDock,
			set: (p) => settings.setConversationDock(p),
			journey: 'conversation-dock'
		}
	]);

	const sideOptions: SegmentOption[] = [
		{ value: 'left', label: m.settings_spawn_dock_side_left() },
		{ value: 'right', label: m.settings_spawn_dock_side_right() }
	];
</script>

<div class="wrap" data-setting-row>
	<table class="docks">
		<thead>
			<tr>
				<th scope="col"><Text variant="eyebrow" tone="muted">{m.set_panels_col_panel()}</Text></th>
				<th scope="col"><Text variant="eyebrow" tone="muted">{m.set_panels_col_shown()}</Text></th>
				<th scope="col"><Text variant="eyebrow" tone="muted">{m.set_panels_col_side()}</Text></th>
				<th scope="col"><Text variant="eyebrow" tone="muted">{m.set_panels_col_width()}</Text></th>
			</tr>
		</thead>
		<tbody>
			{#each docks as d (d.key)}
				<tr data-journey={d.journey ? 'setting' : undefined} data-journey-key={d.journey}>
					<th scope="row"><Text weight="semibold" size="sm">{d.label}</Text></th>
					<td>
						<Switch
							bind:checked={() => d.state.enabled, (v) => d.set({ enabled: v })}
							label={m.set_panels_shown_label({ panel: d.label })}
						/>
					</td>
					<td>
						<SegmentedControl
							options={sideOptions}
							label={m.set_panels_side_label({ panel: d.label })}
							control
							disabled={!d.state.enabled}
							bind:value={() => d.state.side, (v) => d.set({ side: v as SpawnDockSide })}
						/>
					</td>
					<td>
						{#if d.state.width}
							<span class="width">
								<Text size="sm" class="num">{m.set_panels_width_px({ px: Math.round(d.state.width) })}</Text>
								<Button size="sm" variant="ghost" onclick={() => d.set({ width: undefined })}>
									{m.settings_reset()}
								</Button>
							</span>
						{:else}
							<Text size="sm" tone="faint">{m.set_panels_width_default()}</Text>
						{/if}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>

<style>
	.wrap {
		padding: var(--sp-3) var(--sp-4);
		overflow-x: auto;
	}
	.docks {
		width: 100%;
		border-collapse: collapse;
	}
	th,
	td {
		padding: var(--sp-2) var(--sp-3) var(--sp-2) 0;
		text-align: start;
		vertical-align: middle;
		white-space: nowrap;
	}
	tbody tr + tr > * {
		border-top: 1px solid var(--divider, var(--border));
	}
	.width {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
