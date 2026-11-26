<script lang="ts">
	import { Badge, Button, Checkbox, Input, Select, Switch, Text } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import { endpoints } from '#lib/queries/index.ts';
	import { harnessLabel } from '#lib/harnesses.ts';
	import { harnessTable } from '#lib/harnesses.svelte.ts';
	import type { CodexSandbox } from '@bindings/CodexSandbox';
	import type { HarnessAutoupdateInfo } from '@bindings/HarnessAutoupdateInfo';
	import type { HarnessUpdatePolicy } from '@bindings/HarnessUpdatePolicy';
	import type { HarnessVersion } from '@bindings/HarnessVersion';
	import type { HarnessVersions } from '@bindings/HarnessVersions';
	import type { MachineHarnessInfo } from '@bindings/MachineHarnessInfo';
	import { DRAIN_RESTARTABLE, deniedHealth, versionSkew } from '#lib/harnessHealth.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { autoSave, serverSave } from './serverSave.svelte.ts';

	const DEFAULT_HARNESSES = ['claude-code', 'codex'];
	const CODEX_SANDBOX_DOCS =
		'https://github.com/DorskFR/cctui/blob/main/docs/codex-sandbox.md';
	const DEFAULT_POLICY: HarnessUpdatePolicy = {
		enabled: false,
		interval_hours: 24,
		harnesses: [...DEFAULT_HARNESSES]
	};
	const harnessIds = $derived(harnessTable().map((h) => h.id));

	let info = $state<HarnessAutoupdateInfo | null>(null);
	let enabled = $state(false);
	let hours = $state('24');
	let harnesses = $state<string[]>([...DEFAULT_HARNESSES]);

	function apply(next: HarnessAutoupdateInfo) {
		info = next;
		const p = next.instance ?? DEFAULT_POLICY;
		enabled = p.enabled;
		hours = String(p.interval_hours);
		harnesses = [...p.harnesses];
	}

	$effect(() => {
		endpoints
			.harnessAutoupdate()
			.then(apply)
			.catch(() => {});
	});

	const normalize = (p: HarnessUpdatePolicy): HarnessUpdatePolicy => ({
		enabled: p.enabled,
		interval_hours: p.interval_hours,
		harnesses: harnessIds.filter((h) => p.harnesses.includes(h))
	});
	const draft = $derived<HarnessUpdatePolicy>(
		normalize({ enabled, interval_hours: Math.max(1, Math.floor(Number(hours) || 24)), harnesses })
	);
	const dirty = $derived(
		JSON.stringify(draft) !== JSON.stringify(normalize(info?.instance ?? DEFAULT_POLICY))
	);

	async function run(call: () => Promise<HarnessAutoupdateInfo>) {
		const sent = JSON.stringify(draft);
		const res = await serverSave.run(call);
		if (!res) return;
		if (JSON.stringify(draft) === sent) apply(res);
		else info = res;
	}
	autoSave(
		() => (info && dirty ? draft : undefined),
		(next) => run(() => endpoints.setHarnessAutoupdate(next))
	);

	function toggleHarness(h: string, on: boolean) {
		harnesses = on ? [...new Set([...harnesses, h])] : harnesses.filter((x) => x !== h);
	}

	function overrideValue(row: MachineHarnessInfo): string {
		if (!row.policy) return 'inherit';
		return row.policy.enabled ? 'on' : 'off';
	}

	function setOverride(row: MachineHarnessInfo, value: string) {
		const base = info?.instance ?? DEFAULT_POLICY;
		const policy = value === 'inherit' ? null : { ...base, enabled: value === 'on' };
		run(() => endpoints.setMachineHarnessAutoupdate(row.machine_id, policy));
	}

	let restarting = $state<string | null>(null);

	async function restartWhenIdle(row: MachineHarnessInfo, harness: string) {
		restarting = `${row.machine_id}:${harness}`;
		try {
			await endpoints.restartHarness(row.machine_id, harness);
			toasts.ok(m.settings_harness_restart_requested({ harness, name: row.name }));
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			restarting = null;
		}
	}

	function brokenSandbox(
		s: CodexSandbox | null | undefined
	): Exclude<CodexSandbox, { state: 'ok' }> | null {
		return s && s.state !== 'ok' ? s : null;
	}

	function versions(v: HarnessVersion | null | undefined): string {
		if (!v) return '—';
		if (!v.daemon || v.daemon === v.cli) return v.cli ?? '—';
		return `${v.cli ?? '?'} / ${v.daemon}`;
	}

	/** Every harness in the table, then any the machine reported beyond it. */
	function versionLines(all: HarnessVersions): string {
		const extra = Object.keys(all).filter((id) => !harnessIds.includes(id));
		return [...harnessIds, ...extra]
			.map((id) => `${harnessLabel(id, harnessTable())} ${versions(all[id])}`)
			.join(' · ');
	}
</script>

<SettingGroup title={m.settings_harness_update_label()}>
	<SettingRow
		label={m.settings_harness_update_enable()}
		help={m.settings_harness_update_help()}
		server
		admin
		wide
		selfLabelled
	>
		<div class="policy">
			<Switch bind:checked={enabled} label={m.settings_harness_update_enable()} />
			<Input
				bind:value={hours}
				inputmode="numeric"
				aria-label={m.settings_harness_update_interval()}
				placeholder={m.settings_harness_update_interval()}
			/>
			{#each harnessIds as h (h)}
				<Checkbox
					label={h}
					checked={harnesses.includes(h)}
					onchange={(e) => toggleHarness(h, (e.currentTarget as HTMLInputElement).checked)}
				/>
			{/each}
		</div>
	</SettingRow>

	{#if info && info.machines.length > 0}
		<SettingRow label={m.settings_harness_update_machines()} wide selfLabelled>
			<ul class="machines">
				{#each info.machines as row (row.machine_id)}
					<li class="machine">
						<Text size="sm">{row.name}</Text>
						<Select
							value={overrideValue(row)}
							disabled={serverSave.status === 'pending'}
							aria-label={m.settings_harness_update_override({ name: row.name })}
							onchange={(e) => setOverride(row, (e.currentTarget as HTMLSelectElement).value)}
						>
							<option value="inherit">{m.settings_harness_update_inherit()}</option>
							<option value="on">{m.settings_harness_update_on()}</option>
							<option value="off">{m.settings_harness_update_off()}</option>
						</Select>
						{#if row.report}
							<Text size="xs" tone="faint" numeric>
								{versionLines(row.report.versions)}
							</Text>
							{#if row.report.managed_by_image}
								<Badge>{m.settings_harness_update_managed_by_image()}</Badge>
							{/if}
							{#if deniedHealth(row.report)}
								{@const denied = deniedHealth(row.report)!}
								<Badge tone="danger" size="sm" border>
									{m.settings_machine_health_denied({ name: row.name })}
								</Badge>
								<Text size="xs" tone="faint" variant="code">{denied.detail}</Text>
							{/if}
							{#each versionSkew(row.report.versions) as skew (skew.harness)}
								<Badge tone="warn" size="sm" border>
									{m.settings_harness_skew({
										harness: harnessLabel(skew.harness, harnessTable()),
										daemon: skew.daemon,
										cli: skew.cli
									})}
								</Badge>
								{#if DRAIN_RESTARTABLE.has(skew.harness)}
									<Button
										size="sm"
										disabled={serverSave.status === 'pending' || restarting !== null}
										onclick={() => restartWhenIdle(row, skew.harness)}
									>
										{m.settings_harness_restart_when_idle()}
									</Button>
								{/if}
							{/each}
							{#if brokenSandbox(row.report.codex_sandbox)}
								{@const sandbox = brokenSandbox(row.report.codex_sandbox)!}
								<Badge tone="danger" size="sm" border>
									{sandbox.state === 'userns_denied'
										? m.settings_codex_sandbox_userns_denied()
										: m.settings_codex_sandbox_unknown()}
								</Badge>
								<Text size="xs" tone="faint" variant="code">{sandbox.detail}</Text>
								{#if sandbox.state === 'userns_denied'}
									<a href={CODEX_SANDBOX_DOCS} target="_blank" rel="noreferrer noopener">
										<Text size="xs">{m.settings_codex_sandbox_help()}</Text>
									</a>
								{/if}
							{/if}
							{#each row.report.outcomes as o (o.harness)}
								<Text size="xs" tone={o.outcome.startsWith('failed') ? 'danger' : 'faint'}>
									{o.harness}: {o.outcome}
								</Text>
							{/each}
						{:else}
							<Text size="xs" tone="faint">{m.settings_harness_update_no_report()}</Text>
						{/if}
					</li>
				{/each}
			</ul>
		</SettingRow>
	{/if}
</SettingGroup>

<style>
	.policy {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
	.machines {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.machine {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
</style>
