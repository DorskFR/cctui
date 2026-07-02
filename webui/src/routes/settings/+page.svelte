<script lang="ts">
	// User settings (CCT-426, epic CCT-357). Server-persisted via the `settings`
	// singleton (GET/PUT /api/v1/settings, localStorage-mirrored). Grouped into
	// sections — New session · Session list · Display · Notifications · Keyboard —
	// matching the settings catalogue.
	import { settings } from '$lib/settings.svelte';
	import { theme, THEMES } from '$lib/theme.svelte';
	import { fontScale, SCALE_LEVELS } from '$lib/fontscale.svelte';
	import { notify } from '$lib/notify.svelte';
	import { useAllMachines, useDispatchers, useAccounts, useLabels } from '$lib/queries';
	import {
		modes,
		claudeModels,
		codexModels,
		claudeEfforts,
		codexEfforts,
		adapterLabel
	} from '$lib/components/organisms/spawn/options';
	import { Card, Heading, Select, Stack, Switch, Text } from '@dorsk/tsumikit';
	import type { HarnessMode } from '$lib/settings.svelte';

	const machines = useAllMachines(() => true);
	const dispatchers = useDispatchers(() => true);
	const accounts = useAccounts(() => true);
	const labelsQuery = useLabels();

	const machineList = $derived($machines.data ?? []);
	const dispatcherList = $derived($dispatchers.data ?? []);
	const accountList = $derived($accounts.data ?? []);
	const allLabels = $derived($labelsQuery.data?.labels ?? []);

	const ns = $derived(settings.state.newSession);
	const sl = $derived(settings.state.sessionList);

	// Validate saved id references against the live lists so a deleted machine /
	// dispatcher / account default surfaces a subtle hint and falls back gracefully.
	const machineMissing = $derived(
		!!ns.defaultMachineId && machineList.length > 0 && !machineList.some((m) => m.id === ns.defaultMachineId)
	);
	const dispatcherMissing = $derived(
		!!ns.defaultDispatcherId &&
			dispatcherList.length > 0 &&
			!dispatcherList.includes(ns.defaultDispatcherId)
	);
	const accountMissing = $derived(
		!!ns.defaultAccount && accountList.length > 0 && !accountList.some((a) => a.name === ns.defaultAccount)
	);

	const adapterOpts = [
		{ v: 'claude-code', label: adapterLabel('claude-code') },
		{ v: 'codex', label: adapterLabel('codex') }
	];

	// Display section mirrors the live theme/fontScale/notify singletons (the
	// runtime drivers) AND records the value into the settings blob, so the panel
	// is the single surface while behaviour stays driven by those singletons.
	function setTheme(id: string) {
		theme.set(id as typeof theme.current);
		settings.setDisplay({ theme: id });
	}
	function setFontScale(levelId: string) {
		fontScale.set(levelId);
		settings.setDisplay({ fontScale: fontScale.current });
	}
	async function toggleNotify() {
		if (notify.enabled) notify.disable();
		else await notify.enable();
		settings.setDisplay({ notifyEnabled: notify.enabled });
	}
	function toggleNotifySound() {
		notify.setSound(!notify.sound);
		settings.setDisplay({ notifySound: notify.sound });
	}

	// Multi-value (csv) helpers for the label-id sets.
	function csv(ids: string[]): string {
		return ids.join(', ');
	}

	// Claude harness mode (epic CCT-494). Per-user; applies to all the user's
	// machines and a connected daemon switches within ~1s. Codex sessions ignore it.
	const harnessMode = $derived(settings.harnessMode);
	const harnessOpts: { v: HarnessMode; label: string; help: string }[] = [
		{
			v: 'bg',
			label: 'Background (default)',
			help: 'Full live fidelity with native FleetView — live PTY, mid-turn control.'
		},
		{
			v: 'sdk',
			label: 'SDK',
			help: 'Persistent, structured session. No PTY.'
		},
		{
			v: 'oneshot',
			label: 'One-shot',
			help: 'Ephemeral, per-turn. No live mid-turn control.'
		}
	];
	const harnessHelp = $derived(harnessOpts.find((o) => o.v === harnessMode)?.help ?? '');
</script>

<Stack gap="lg">
	<header class="head">
		<Heading level={1}>Settings</Heading>
		<Text tone="faint">Your preferences, saved to your account.</Text>
	</header>

	<!-- ── New session ──────────────────────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>New session</Heading>
			<dl class="props">
				<div class="prop">
					<dt>
						<Text weight="semibold">Remember last used</Text>
						<Text size="sm" tone="faint">
							When on, a new session reuses your last spawn config and these defaults
							only fill the gaps. When off, new sessions start from these defaults.
						</Text>
					</dt>
					<dd>
						<Switch
							checked={ns.rememberLastUsed}
							label="Remember last used"
							onclick={() => settings.setNewSession({ rememberLastUsed: !ns.rememberLastUsed })}
						/>
					</dd>
				</div>

				<div class="prop">
					<dt><Text weight="semibold">Default target</Text></dt>
					<dd>
						<Select
							value={ns.defaultTarget}
							onchange={(e) =>
								settings.setNewSession({
									defaultTarget: (e.currentTarget as HTMLSelectElement).value as 'machine' | 'dispatch'
								})}
						>
							<option value="machine">Machine</option>
							<option value="dispatch">Dispatch (k8s)</option>
						</Select>
					</dd>
				</div>

				<div class="prop">
					<dt>
						<Text weight="semibold">Default machine</Text>
						{#if machineMissing}
							<Text size="sm" tone="danger">Your default machine is no longer available.</Text>
						{/if}
					</dt>
					<dd>
						<Select
							value={ns.defaultMachineId ?? ''}
							onchange={(e) =>
								settings.setNewSession({
									defaultMachineId: (e.currentTarget as HTMLSelectElement).value || null
								})}
						>
							<option value="">First available</option>
							{#each machineList as m (m.id)}
								<option value={m.id}>{m.display_name || m.name}</option>
							{/each}
						</Select>
					</dd>
				</div>

				<div class="prop">
					<dt>
						<Text weight="semibold">Default dispatcher</Text>
						{#if dispatcherMissing}
							<Text size="sm" tone="danger">Your default dispatcher is no longer available.</Text>
						{/if}
					</dt>
					<dd>
						<Select
							value={ns.defaultDispatcherId ?? ''}
							onchange={(e) =>
								settings.setNewSession({
									defaultDispatcherId: (e.currentTarget as HTMLSelectElement).value || null
								})}
						>
							<option value="">First available</option>
							{#each dispatcherList as d (d)}
								<option value={d}>{d}</option>
							{/each}
						</Select>
					</dd>
				</div>

				<div class="prop">
					<dt><Text weight="semibold">Default adapter</Text></dt>
					<dd>
						<Select
							value={ns.defaultAdapter}
							onchange={(e) =>
								settings.setNewSession({ defaultAdapter: (e.currentTarget as HTMLSelectElement).value })}
						>
							{#each adapterOpts as a (a.v)}
								<option value={a.v}>{a.label}</option>
							{/each}
						</Select>
					</dd>
				</div>

				<div class="prop">
					<dt><Text weight="semibold">Default Claude model</Text></dt>
					<dd>
						<Select
							value={ns.defaultModelClaude}
							onchange={(e) =>
								settings.setNewSession({
									defaultModelClaude: (e.currentTarget as HTMLSelectElement).value
								})}
						>
							{#each claudeModels as m (m.v)}
								<option value={m.v}>{m.label}</option>
							{/each}
						</Select>
					</dd>
				</div>

				<div class="prop">
					<dt><Text weight="semibold">Default Codex model</Text></dt>
					<dd>
						<Select
							value={ns.defaultModelCodex}
							onchange={(e) =>
								settings.setNewSession({
									defaultModelCodex: (e.currentTarget as HTMLSelectElement).value
								})}
						>
							{#each codexModels as m (m.v)}
								<option value={m.v}>{m.label}</option>
							{/each}
						</Select>
					</dd>
				</div>

				<div class="prop">
					<dt><Text weight="semibold">Default Claude effort</Text></dt>
					<dd>
						<Select
							value={ns.defaultEffortClaude}
							onchange={(e) =>
								settings.setNewSession({
									defaultEffortClaude: (e.currentTarget as HTMLSelectElement).value
								})}
						>
							{#each claudeEfforts as ef (ef)}
								<option value={ef}>{ef || 'Default'}</option>
							{/each}
						</Select>
					</dd>
				</div>

				<div class="prop">
					<dt><Text weight="semibold">Default Codex effort</Text></dt>
					<dd>
						<Select
							value={ns.defaultEffortCodex}
							onchange={(e) =>
								settings.setNewSession({
									defaultEffortCodex: (e.currentTarget as HTMLSelectElement).value
								})}
						>
							{#each codexEfforts as ef (ef)}
								<option value={ef}>{ef || 'Default'}</option>
							{/each}
						</Select>
					</dd>
				</div>

				<div class="prop">
					<dt><Text weight="semibold">Default permission mode</Text></dt>
					<dd>
						<Select
							value={ns.defaultPermissionMode}
							onchange={(e) =>
								settings.setNewSession({
									defaultPermissionMode: (e.currentTarget as HTMLSelectElement).value
								})}
						>
							<option value="">Default</option>
							{#each modes as m (m.v)}
								<option value={m.v}>{m.label}</option>
							{/each}
						</Select>
					</dd>
				</div>

				<div class="prop">
					<dt>
						<Text weight="semibold">Default account</Text>
						{#if accountMissing}
							<Text size="sm" tone="danger">Your default account is no longer available.</Text>
						{/if}
					</dt>
					<dd>
						<Select
							value={ns.defaultAccount ?? ''}
							onchange={(e) =>
								settings.setNewSession({
									defaultAccount: (e.currentTarget as HTMLSelectElement).value || null
								})}
						>
							<option value="">Default (no account)</option>
							{#each accountList as a (a.id)}
								<option value={a.name}>{a.name}</option>
							{/each}
						</Select>
					</dd>
				</div>

				<div class="prop">
					<dt>
						<Text weight="semibold">Default labels</Text>
						<Text size="sm" tone="faint">
							{ns.defaultLabels.length
								? allLabels
										.filter((l) => ns.defaultLabels.includes(l.id))
										.map((l) => l.name)
										.join(', ') || '—'
								: 'None'}
						</Text>
					</dt>
					<dd></dd>
				</div>
			</dl>
		</Stack>
	</Card>

	<!-- ── Session list ─────────────────────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>Session list</Heading>
			<dl class="props">
				<div class="prop">
					<dt><Text weight="semibold">Sort</Text></dt>
					<dd>
						<Select
							value={sl.sort}
							onchange={(e) =>
								settings.setSessionList({
									sort: (e.currentTarget as HTMLSelectElement).value as typeof sl.sort
								})}
						>
							<option value="activity">Recent activity</option>
							<option value="created">Created</option>
							<option value="name">Name</option>
						</Select>
					</dd>
				</div>
				<div class="prop">
					<dt><Text weight="semibold">View</Text></dt>
					<dd>
						<Select
							value={sl.view}
							onchange={(e) =>
								settings.setSessionList({
									view: (e.currentTarget as HTMLSelectElement).value as typeof sl.view
								})}
						>
							<option value="list">List</option>
							<option value="card">Cards</option>
						</Select>
					</dd>
				</div>
				<div class="prop">
					<dt><Text weight="semibold">Density</Text></dt>
					<dd>
						<Select
							value={sl.density}
							onchange={(e) =>
								settings.setSessionList({
									density: (e.currentTarget as HTMLSelectElement).value as typeof sl.density
								})}
						>
							<option value="normal">Detailed</option>
							<option value="compact">Compact</option>
						</Select>
					</dd>
				</div>
				<div class="prop">
					<dt>
						<Text weight="semibold">Sections</Text>
						<Text size="sm" tone="faint">{sl.section || 'All'}</Text>
					</dt>
					<dd></dd>
				</div>
				<div class="prop">
					<dt>
						<Text weight="semibold">Label filter</Text>
						<Text size="sm" tone="faint">{csv(sl.labelFilter) || 'None'}</Text>
					</dt>
					<dd></dd>
				</div>
			</dl>
		</Stack>
	</Card>

	<!-- ── Display ──────────────────────────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>Display</Heading>
			<dl class="props">
				<div class="prop">
					<dt><Text weight="semibold">Theme</Text></dt>
					<dd>
						<Select
							value={theme.current}
							onchange={(e) => setTheme((e.currentTarget as HTMLSelectElement).value)}
						>
							{#each THEMES as t (t.id)}
								<option value={t.id}>{t.icon} {t.label}</option>
							{/each}
						</Select>
					</dd>
				</div>
				<div class="prop">
					<dt><Text weight="semibold">Font size</Text></dt>
					<dd>
						<Select
							value={fontScale.levelId}
							onchange={(e) => setFontScale((e.currentTarget as HTMLSelectElement).value)}
						>
							{#each SCALE_LEVELS as l (l.id)}
								<option value={l.id}>{l.label}</option>
							{/each}
						</Select>
					</dd>
				</div>
				<div class="prop">
					<dt>
						<Text weight="semibold">Archive shortcut</Text>
						<Text size="sm" tone="faint">
							In an open conversation, ⌘ E (Mac) / Ctrl + E interrupts any running turn
							and archives the session.
						</Text>
					</dt>
					<dd>
						<Switch
							checked={settings.state.display.archiveShortcut}
							label="Archive shortcut"
							onclick={() => settings.toggleArchiveShortcut()}
						/>
					</dd>
				</div>
			</dl>
		</Stack>
	</Card>

	<!-- ── Claude harness mode (epic CCT-494) ───────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>Claude harness mode</Heading>
			<dl class="props">
				<div class="prop">
					<dt>
						<Text weight="semibold">Execution mode</Text>
						<Text size="sm" tone="faint">{harnessHelp}</Text>
						<Text size="sm" tone="faint">
							Applies to all your machines and takes effect within ~1s. Only affects
							Claude sessions — Codex sessions ignore this.
						</Text>
					</dt>
					<dd>
						<Select
							value={harnessMode}
							onchange={(e) =>
								settings.setHarnessMode((e.currentTarget as HTMLSelectElement).value as HarnessMode)}
						>
							{#each harnessOpts as o (o.v)}
								<option value={o.v}>{o.label}</option>
							{/each}
						</Select>
					</dd>
				</div>
			</dl>
		</Stack>
	</Card>

	<!-- ── Notifications ────────────────────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>Notifications</Heading>
			<dl class="props">
				<div class="prop">
					<dt>
						<Text weight="semibold">Notify on input needed</Text>
						<Text size="sm" tone="faint">
							A browser notification when a session is waiting for you.
						</Text>
					</dt>
					<dd>
						<Switch
							checked={notify.enabled}
							label="Notifications"
							disabled={!notify.supported}
							onclick={() => void toggleNotify()}
						/>
					</dd>
				</div>
				<div class="prop">
					<dt><Text weight="semibold">Sound</Text></dt>
					<dd>
						<Switch checked={notify.sound} label="Notification sound" onclick={toggleNotifySound} />
					</dd>
				</div>
			</dl>
		</Stack>
	</Card>

	<!-- ── Keyboard ─────────────────────────────────────────────────────── -->
	<Card>
		<Stack gap="md">
			<Heading level={2}>Keyboard</Heading>
			<Text tone="faint">Custom keyboard shortcuts are coming soon.</Text>
		</Stack>
	</Card>
</Stack>

<style>
	.head {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.props {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		margin: 0;
	}
	.prop {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--sp-3);
	}
	.prop dt {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
	.prop dd {
		margin: 0;
		flex: none;
	}
	.prop + .prop {
		border-top: 1px solid var(--border);
		padding-top: var(--sp-3);
	}
</style>
