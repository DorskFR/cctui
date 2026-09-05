<script lang="ts">
	import { Button, Field, Input, Select, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import Error from '$lib/components/atoms/Error.svelte';
	import { looseSettings } from './pages.logic';

	let {
		endpoint = false,
		rawJson: showRaw = false,
		settableKeys = new Set<string>(),
		catalogReady = false,
		claimed = new Set<string>(),
		baseUrl = $bindable(''),
		credential = $bindable(''),
		authScheme = $bindable('keep'),
		settings = $bindable({}),
		moveTargets = [],
		moveFamily = '',
		onmove
	}: {
		/** Static-credential providers can rewrite their endpoint here. */
		endpoint?: boolean;
		rawJson?: boolean;
		settableKeys?: Set<string>;
		catalogReady?: boolean;
		/** Settings names some page already renders as a row. */
		claimed?: Set<string>;
		baseUrl?: string;
		credential?: string;
		authScheme?: 'bearer' | 'api_key' | 'keep';
		settings?: Record<string, unknown>;
		moveTargets?: { id: string; name: string }[];
		moveFamily?: string;
		onmove?: (targetId: string) => void;
	} = $props();

	let raw = $state('');
	let rawError = $state('');
	let moveTarget = $state('');

	const loose = $derived(looseSettings(settings, claimed));

	function applyRaw() {
		rawError = '';
		const text = raw.trim();
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
		if (!catalogReady) {
			rawError = m.providers_raw_catalog_loading();
			return;
		}
		const obj = parsed as Record<string, unknown>;
		const bad = Object.keys(obj).filter((k) => !settableKeys.has(k));
		if (bad.length) {
			rawError = m.providers_raw_not_settable({ keys: bad.join(', ') });
			return;
		}
		settings = { ...settings, ...obj };
		raw = '';
	}

	function clearLoose(name: string) {
		const next = { ...settings };
		delete next[name];
		settings = next;
	}
</script>

<div class="page">
	{#if endpoint}
		<div class="block">
			<Text as="div" tone="muted" size="sm">{m.accounts_endpoint_label()}</Text>
			<Text as="div" tone="faint" size="xs">{m.accounts_endpoint_help()}</Text>
			<Field label={m.accounts_field_base_url()}>
				<Input bind:value={baseUrl} mono placeholder={m.accounts_placeholder_keep_current()} />
			</Field>
			<Field label={m.accounts_field_auth_scheme()}>
				<Select bind:value={authScheme}>
					<option value="keep">{m.accounts_auth_keep()}</option>
					<option value="bearer">{m.accounts_auth_bearer()}</option>
					<option value="api_key">{m.accounts_auth_api_key()}</option>
				</Select>
			</Field>
			<Field label={m.accounts_field_credential()}>
				<Input
					type="password"
					bind:value={credential}
					placeholder={m.accounts_placeholder_keep_current()}
				/>
			</Field>
		</div>
	{/if}

	{#if showRaw}
		<div class="block">
			<Text as="div" tone="muted" size="sm">{m.providers_advanced_title()}</Text>
			<Text as="div" tone="faint" size="xs">
				{m.providers_advanced_help_before()}
				<Text variant="code">"editorMode": "vim"</Text>{m.providers_advanced_help_after()}
			</Text>
			{#each loose as [k, v] (k)}
				<div class="loose">
					<Text variant="code" size="xs">{k}: {JSON.stringify(v)}</Text>
					<Button size="sm" onclick={() => clearLoose(k)} aria-label={m.providers_remove_key_aria({ key: k })}>✕</Button>
				</div>
			{/each}
			<Input
				bind:value={raw}
				placeholder={'{ "editorMode": "vim" }'}
				aria-label={m.a11y_advanced_json()}
				mono
				onkeydown={(e: KeyboardEvent) => e.key === 'Enter' && applyRaw()}
			/>
			{#if rawError}<Error>{rawError}</Error>{/if}
			<div>
				<Button size="sm" onclick={applyRaw} disabled={!raw.trim()}>{m.providers_merge_json()}</Button>
			</div>
		</div>
	{/if}

	{#if moveTargets.length}
		<div class="block">
			<Text as="div" tone="muted" size="sm">{m.accounts_move_label()}</Text>
			<Text as="div" tone="faint" size="xs">{m.accounts_move_help({ family: moveFamily })}</Text>
			<div class="move">
				<Select bind:value={moveTarget} aria-label={m.accounts_move_label()}>
					<option value="">{m.accounts_move_pick()}</option>
					{#each moveTargets as t (t.id)}
						<option value={t.id}>{t.name}</option>
					{/each}
				</Select>
				<Button size="sm" disabled={!moveTarget} onclick={() => onmove?.(moveTarget)}>
					{m.accounts_move_button()}
				</Button>
			</div>
		</div>
	{/if}
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
	}
	.block {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.loose {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}
	.move {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		gap: var(--sp-2);
		align-items: center;
	}
</style>
