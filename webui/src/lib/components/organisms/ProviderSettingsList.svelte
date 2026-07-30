<script lang="ts">
	// Compact unified per-provider settings list. Every curated knob —
	// settings.json boolean keys AND curated env vars — renders as a single row
	// (label + muted key/env name + care chip on the left; a compact control on
	// the right). Env vars that alias a settings key merge into one row that
	// writes the settings key; env-only vars read/write `settings.env[NAME]`.
	//
	// All state persists in the provider `settings_json` (the account env blob is
	// write-only and can't round-trip a toggle), so reopening the modal shows the
	// same choices. The raw-JSON box stays as the escape hatch for allowlisted
	// non-curated keys.
	import { Button, Input, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import Error from '$lib/components/atoms/Error.svelte';
	import SegmentedControl from '$lib/components/molecules/SegmentedControl.svelte';
	import { useSettingsCatalog } from '$lib/queries';
	import type { EnvVar } from '$lib/bindings/EnvVar';

	let {
		settings = $bindable({})
	}: {
		settings?: Record<string, unknown>;
	} = $props();

	type Control = 'tristate' | 'toggle' | 'enum' | 'number' | 'string';
	interface Knob {
		id: string;
		label: string;
		sub: string;
		care: boolean;
		control: Control;
		loc: 'setting' | 'env';
		name: string;
		values?: string[];
		// For env-derived knobs that write a settings key: values the settings
		// key's schema enum accepts. Anything else falls back to the env var
		// (mirrors the daemon's effortLevel/CLAUDE_CODE_EFFORT_LEVEL split).
		envName?: string;
		settingsValues?: string[];
		note: string;
	}
	interface Group {
		title: string;
		knobs: Knob[];
	}

	const TRI = [
		{ value: '', label: m.providers_opt_default() },
		{ value: 'true', label: m.providers_opt_on() },
		{ value: 'false', label: m.providers_opt_off() }
	];
	const FLAG = [
		{ value: '', label: m.providers_opt_default() },
		{ value: '1', label: m.providers_opt_on() }
	];

	const catalog = useSettingsCatalog();
	const catalogKeys = $derived($catalog.data?.keys ?? []);
	const catalogEnv = $derived<EnvVar[]>($catalog.data?.env ?? []);
	const boolKeys = $derived(catalogKeys.filter((k) => k.group !== null));
	const keyNames = $derived(new Set(catalogKeys.map((k) => k.name)));

	// Curated env vars keyed by the settings key they alias, so a boolean row can
	// show its env twin as a subtitle and we can drop the env's own row.
	const envBySettingsEquiv = $derived(
		new Map(catalogEnv.filter((e) => e.settings_equiv).map((e) => [e.settings_equiv as string, e]))
	);

	function boolKnob(name: string, label: string, care: boolean, note: string): Knob {
		const twin = envBySettingsEquiv.get(name);
		return {
			id: `s:${name}`,
			label,
			sub: twin ? `${name} · ${twin.name}` : name,
			care,
			control: 'tristate',
			loc: 'setting',
			name,
			note
		};
	}

	function envKnob(e: EnvVar): Knob {
		const control: Control =
			e.kind === 'flag'
				? 'toggle'
				: e.kind === 'number'
					? 'number'
					: e.kind === 'enum'
						? 'enum'
						: 'string';
		const loc = e.settings_equiv ? 'setting' : 'env';
		const equivKey = e.settings_equiv
			? catalogKeys.find((k) => k.name === e.settings_equiv)
			: undefined;
		return {
			id: `e:${e.name}`,
			label: e.label ?? e.name,
			sub: e.settings_equiv ? `${e.name} · ${e.settings_equiv}` : e.name,
			care: e.tag === 'care',
			control,
			loc,
			name: e.settings_equiv ?? e.name,
			values: e.values ?? undefined,
			envName: e.settings_equiv ? e.name : undefined,
			settingsValues: equivKey?.enum ? equivKey.enum.split(',').map((v) => v.trim()) : undefined,
			note: e.notes ?? ''
		};
	}

	// Settings-boolean groups first (with any env twin merged in), then the
	// remaining curated env vars grouped by their catalog group. Env vars that
	// are exact aliases (DO_NOT_TRACK) or that merged into a boolean row are
	// dropped so each knob renders exactly once.
	const groups = $derived.by<Group[]>(() => {
		if (!$catalog.data) return [];
		const out: Group[] = [];
		const settingGroups = [...new Set(boolKeys.map((k) => k.group as string))];
		for (const g of settingGroups) {
			out.push({
				title: g,
				knobs: boolKeys
					.filter((k) => k.group === g)
					.map((k) => boolKnob(k.name, k.label ?? k.name, k.tag === 'care', k.notes ?? ''))
			});
		}
		const boolNames = new Set(boolKeys.map((k) => k.name));
		const remainingEnv = catalogEnv.filter(
			(e) => !e.env_alias_of && !(e.settings_equiv && boolNames.has(e.settings_equiv))
		);
		const envGroups = [...new Set(remainingEnv.map((e) => e.group))];
		for (const g of envGroups) {
			out.push({
				title: g,
				knobs: remainingEnv.filter((e) => e.group === g).map(envKnob)
			});
		}
		return out;
	});

	// --- value accessors (all persisting in `settings`) ------------------------
	function envObj(): Record<string, string> {
		const e = settings.env;
		return e && typeof e === 'object' && !Array.isArray(e) ? (e as Record<string, string>) : {};
	}
	function setSetting(name: string, v: unknown) {
		const next = { ...settings };
		if (v === undefined || v === '') delete next[name];
		else next[name] = v;
		settings = next;
	}
	function setEnv(name: string, v: string) {
		const env = { ...envObj() };
		if (v === '') delete env[name];
		else env[name] = v;
		const next = { ...settings };
		if (Object.keys(env).length) next.env = env;
		else delete next.env;
		settings = next;
	}
	function getKnob(k: Knob): string {
		if (k.control === 'tristate') {
			const v = settings[k.name];
			return v === true ? 'true' : v === false ? 'false' : '';
		}
		if (k.loc === 'setting') {
			const v = settings[k.name];
			if (typeof v === 'string') return v;
			return k.envName ? (envObj()[k.envName] ?? '') : '';
		}
		return envObj()[k.name] ?? '';
	}
	function setKnob(k: Knob, v: string) {
		if (k.control === 'tristate') {
			setSetting(k.name, v === 'true' ? true : v === 'false' ? false : undefined);
		} else if (k.loc === 'setting') {
			if (k.envName && k.settingsValues && v !== '' && !k.settingsValues.includes(v)) {
				setSetting(k.name, undefined);
				setEnv(k.envName, v);
			} else {
				setSetting(k.name, v);
				if (k.envName) setEnv(k.envName, '');
			}
		} else {
			setEnv(k.name, v);
		}
	}
	function enumOptions(k: Knob) {
		return [{ value: '', label: m.providers_opt_default() }, ...(k.values ?? []).map((v) => ({ value: v, label: v }))];
	}

	// --- advanced raw-JSON escape hatch ---------------------------------------
	let rawJson = $state('');
	let rawError = $state('');
	function applyRawJson() {
		rawError = '';
		const text = rawJson.trim();
		if (!text) return;
		let parsed: unknown;
		try {
			parsed = JSON.parse(text);
		} catch (e) {
			rawError = m.providers_raw_invalid_json({ message: (e as globalThis.Error).message });
			return;
		}
		if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
			rawError = m.providers_raw_must_be_object();
			return;
		}
		const obj = parsed as Record<string, unknown>;
		if (!$catalog.data) {
			rawError = m.providers_raw_catalog_loading();
			return;
		}
		const bad = Object.keys(obj).filter((k) => !keyNames.has(k));
		if (bad.length) {
			rawError = m.providers_raw_not_settable({ keys: bad.join(', ') });
			return;
		}
		settings = { ...settings, ...obj };
		rawJson = '';
	}

	// Settings keys reached only via the raw box (not a curated knob and not the
	// knob-managed `env` block) — shown so advanced values in effect are visible.
	const knobSettingNames = $derived(
		new Set([
			...boolKeys.map((k) => k.name),
			...catalogEnv.filter((e) => e.settings_equiv).map((e) => e.settings_equiv as string),
			'env'
		])
	);
	const advancedEntries = $derived(
		Object.entries(settings).filter(([k]) => !knobSettingNames.has(k))
	);
	function clearAdvancedKey(name: string) {
		const next = { ...settings };
		delete next[name];
		settings = next;
	}

	function applyQuietDefaults() {
		const preset = $catalog.data?.preset;
		if (!preset) return;
		const env = { ...envObj(), ...preset.env };
		settings = { ...settings, ...preset.settings, env };
	}
</script>

<div class="settings-editor">
	<div class="head">
		<Text as="div" weight="semibold" size="sm">{m.providers_settings_title()}</Text>
		<Button onclick={applyQuietDefaults} disabled={!$catalog.data}>{m.providers_quiet_defaults()}</Button>
	</div>
	<Text as="p" tone="faint" size="xs">
		{m.providers_settings_help()}
	</Text>

	{#if !$catalog.data}
		<Text as="div" tone="faint" size="xs">
			{$catalog.error ? m.providers_catalog_load_failed() : m.providers_catalog_loading()}
		</Text>
	{:else}
		{#each groups as group (group.title)}
			<div class="group">
				<div class="group-title"><Text as="span" tone="faint" size="xs">{group.title}</Text></div>
				{#each group.knobs as k (k.id)}
					<div class="knob-row">
						<div class="knob-meta">
							<Text as="div" size="sm">
								{k.label}
								{#if k.care}<span class="care" title={m.providers_care_title()}>{m.providers_care()}</span>{/if}
							</Text>
							<div class="knob-sub"><Text as="span" tone="faint" size="xs">{k.sub}</Text></div>
						</div>
						<div class="knob-control">
							{#if k.control === 'tristate'}
								<SegmentedControl
									value={getKnob(k)}
									options={TRI}
									label={k.label}
									onchange={(v) => setKnob(k, v)}
								/>
							{:else if k.control === 'toggle'}
								<SegmentedControl
									value={getKnob(k)}
									options={FLAG}
									label={k.label}
									onchange={(v) => setKnob(k, v)}
								/>
							{:else if k.control === 'enum'}
								<SegmentedControl
									value={getKnob(k)}
									options={enumOptions(k)}
									label={k.label}
									onchange={(v) => setKnob(k, v)}
								/>
							{:else}
								<Input
									value={getKnob(k)}
									oninput={(e: Event) => setKnob(k, (e.currentTarget as HTMLInputElement).value)}
									type={k.control === 'number' ? 'number' : 'text'}
									mono
									placeholder={m.providers_opt_default()}
									aria-label={k.label}
								/>
							{/if}
						</div>
					</div>
				{/each}
			</div>
		{/each}

		<div class="group">
			<Text as="div" tone="muted" size="sm">{m.providers_advanced_title()}</Text>
			<Text as="div" tone="faint" size="xs">
				{m.providers_advanced_help_before()}
				<Text variant="code">"editorMode": "vim"</Text>{m.providers_advanced_help_after()}
			</Text>
			{#if advancedEntries.length}
				<div class="adv-list">
					{#each advancedEntries as [k, v] (k)}
						<div class="adv-item">
							<Text variant="code" size="xs">{k}: {JSON.stringify(v)}</Text>
							<Button onclick={() => clearAdvancedKey(k)} aria-label={m.providers_remove_key_aria({ key: k })}>✕</Button>
						</div>
					{/each}
				</div>
			{/if}
			<Input
				bind:value={rawJson}
				placeholder={'{ "editorMode": "vim" }'}
				mono
				onkeydown={(e: KeyboardEvent) => e.key === 'Enter' && applyRawJson()}
			/>
			{#if rawError}<Error>{rawError}</Error>{/if}
			<Button onclick={applyRawJson} disabled={!rawJson.trim()}>{m.providers_merge_json()}</Button>
		</div>
	{/if}
</div>

<style>
	.settings-editor {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		border-top: 1px solid var(--border);
		padding-top: var(--sp-3);
	}
	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.group-title {
		text-transform: uppercase;
		letter-spacing: 0.04em;
		margin-top: var(--sp-1);
	}
	.knob-row {
		display: grid;
		grid-template-columns: 1fr minmax(0, auto);
		gap: var(--sp-2) var(--sp-3);
		align-items: center;
		padding: var(--sp-1) 0;
	}
	.knob-meta {
		min-width: 0;
	}
	.knob-sub {
		font-family: var(--font-mono, monospace);
		overflow-wrap: anywhere;
	}
	.knob-control {
		justify-self: end;
		min-width: 0;
	}
	.care {
		display: inline-block;
		margin-left: var(--sp-1);
		padding: 0 0.35em;
		border-radius: var(--r-sm);
		font-size: var(--fs-xs);
		background: color-mix(in srgb, var(--warn) 18%, transparent);
		color: var(--warn);
	}
	.adv-list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.adv-item {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}
	/* Collapse to two lines on a narrow modal so the control never overlaps the
	   label. */
	@media (max-width: 30rem) {
		.knob-row {
			grid-template-columns: 1fr;
		}
		.knob-control {
			justify-self: start;
		}
	}
</style>
