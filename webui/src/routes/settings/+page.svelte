<script lang="ts">
	// User settings (CCT-426, epic CCT-357). Server-persisted via the `settings`
	// singleton (GET/PUT /api/v1/settings, localStorage-mirrored). Grouped into
	// sections — Session list · Display · Harness · Notifications · Keyboard —
	// matching the settings catalogue. New-session launch defaults were removed
	// in CCT-563: the per-(machine, cwd) spawn memory (CCT-561) supersedes them.
	import { settings } from '$lib/settings.svelte';
	import { theme, THEMES } from '$lib/theme.svelte';
	import { fontScale, SCALE_LEVELS } from '$lib/fontscale.svelte';
	import { notify } from '$lib/notify.svelte';
	import { Card, Heading, Select, Stack, Switch, Text } from '@dorsk/tsumikit';
	import type { HarnessMode } from '$lib/settings.svelte';

	const sl = $derived(settings.state.sessionList);

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
