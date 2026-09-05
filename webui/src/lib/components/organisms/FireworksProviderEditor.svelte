<script lang="ts">
	// Fireworks credential editor: the gateway settings it injects on every
	// request, and the account-owned model catalog (ids + display names +
	// per-Mtok pricing) that spawn resolves the model from.
	import { Button, Field, Input, Select, Switch, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { AccountModel } from '$lib/queries';

	let {
		settings = $bindable({}),
		models = $bindable([]),
		section = 'all'
	}: {
		/** Render only the gateway knobs or only the catalog (drawer pages). */
		section?: 'all' | 'gateway' | 'models';
		/** Mirrors the provider's `provider_settings` blob. */
		settings?: Record<string, unknown>;
		/** Mirrors the provider's `models` catalog. */
		models?: AccountModel[];
	} = $props();

	const CLX_BEHAVIORS = ['error', 'truncate'];

	const clx = $derived(
		typeof settings.context_length_exceeded_behavior === 'string'
			? settings.context_length_exceeded_behavior
			: settings.context_length_exceeded_behavior === null
				? ''
				: 'error'
	);
	const affinity = $derived(settings.session_affinity !== false);

	function setClx(v: string) {
		settings = { ...settings, context_length_exceeded_behavior: v === '' ? null : v };
	}
	function setAffinity(v: boolean) {
		settings = { ...settings, session_affinity: v };
	}

	function addModel() {
		models = [...models, { model: '', label: '' }];
	}
	function removeModel(i: number) {
		models = models.filter((_, j) => j !== i);
	}
	/** Empty input clears the price rather than storing 0 — an unpriced model is
	 *  not a free one. */
	function num(v: string): number | null {
		const t = v.trim();
		if (!t) return null;
		const n = Number(t);
		return Number.isFinite(n) ? n : null;
	}
	function setField(i: number, key: keyof AccountModel, value: string) {
		const next = [...models];
		const entry = { ...next[i] };
		if (key === 'model' || key === 'label') entry[key] = value;
		else if (key === 'context_length') entry.context_length = num(value);
		else if (key === 'price_input_per_mtok') entry.price_input_per_mtok = num(value);
		else if (key === 'price_cached_input_per_mtok') entry.price_cached_input_per_mtok = num(value);
		else if (key === 'price_output_per_mtok') entry.price_output_per_mtok = num(value);
		next[i] = entry;
		models = next;
	}
	const show = (v: number | null | undefined) => (v === null || v === undefined ? '' : String(v));
</script>

{#if section !== 'models'}
<div class="block">
	<Text as="div" tone="muted" size="sm">{m.fireworks_settings_label()}</Text>
	<Text as="div" tone="faint" size="xs">{m.fireworks_settings_help()}</Text>
	<Field label={m.fireworks_clx_label()}>
		<Select value={clx} onchange={(e) => setClx((e.currentTarget as HTMLSelectElement).value)}>
			<option value="">{m.fireworks_clx_unset()}</option>
			{#each CLX_BEHAVIORS as b (b)}
				<option value={b}>{b}</option>
			{/each}
		</Select>
	</Field>
	<div class="switch-row">
		<Switch
			bind:checked={() => affinity, (v) => setAffinity(v)}
			label={m.fireworks_affinity_label()}
		/>
		<Text as="div" tone="faint" size="xs">{m.fireworks_affinity_help()}</Text>
	</div>
</div>
{/if}

{#if section !== 'gateway'}
<div class="block">
	<Text as="div" tone="muted" size="sm">{m.fireworks_catalog_label()}</Text>
	<Text as="div" tone="faint" size="xs">{m.fireworks_catalog_help()}</Text>
	{#each models as row, i (i)}
		<div class="model-card">
			<div class="ids">
				<Input
					value={row.model}
					placeholder="accounts/fireworks/models/…"
					aria-label={m.a11y_model_code()}
					oninput={(e) => setField(i, 'model', (e.currentTarget as HTMLInputElement).value)}
				/>
				<Input
					value={row.label}
					placeholder={m.accounts_placeholder_model_label()}
					aria-label={m.a11y_model_label()}
					oninput={(e) => setField(i, 'label', (e.currentTarget as HTMLInputElement).value)}
				/>
				<Button variant="danger" onclick={() => removeModel(i)}>✕</Button>
			</div>
			<div class="prices">
				<Field label={m.fireworks_price_in()}>
					<Input
						value={show(row.price_input_per_mtok)}
						inputmode="decimal"
						oninput={(e) =>
							setField(i, 'price_input_per_mtok', (e.currentTarget as HTMLInputElement).value)}
					/>
				</Field>
				<Field label={m.fireworks_price_cached()}>
					<Input
						value={show(row.price_cached_input_per_mtok)}
						inputmode="decimal"
						oninput={(e) =>
							setField(
								i,
								'price_cached_input_per_mtok',
								(e.currentTarget as HTMLInputElement).value
							)}
					/>
				</Field>
				<Field label={m.fireworks_price_out()}>
					<Input
						value={show(row.price_output_per_mtok)}
						inputmode="decimal"
						oninput={(e) =>
							setField(i, 'price_output_per_mtok', (e.currentTarget as HTMLInputElement).value)}
					/>
				</Field>
				<Field label={m.fireworks_context_length()}>
					<Input
						value={show(row.context_length)}
						inputmode="numeric"
						oninput={(e) =>
							setField(i, 'context_length', (e.currentTarget as HTMLInputElement).value)}
					/>
				</Field>
			</div>
		</div>
	{/each}
	<Button onclick={addModel}>{m.fireworks_add_model()}</Button>
</div>
{/if}

<style>
	.block {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.switch-row {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.model-card {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
	}
	.ids {
		display: grid;
		grid-template-columns: 2fr 1fr auto;
		gap: var(--sp-2);
		align-items: center;
	}
	.prices {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(8rem, 1fr));
		gap: var(--sp-2);
	}
</style>
